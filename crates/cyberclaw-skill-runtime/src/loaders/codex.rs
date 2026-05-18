//! Codex Skill 格式加载器
//!
//! Codex Skill 格式：
//! - 使用 `manifest.yaml` 作为元数据文件
//! - 可选的 `README.md` 包含文档
//! - 可选的 `scripts/` 目录包含可执行脚本

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;

use crate::error::SkillRuntimeError;
use crate::handler::SkillHandler;
use crate::loaders::{FormatLoader, LoadedSkill, SkillMetadata};
use cyberclaw_core::ids::SkillId;

/// Codex Skill 加载器
pub struct CodexSkillLoader {
    /// 加载器名称
    name: String,
}

impl CodexSkillLoader {
    /// 创建新的 Codex Skill 加载器
    pub fn new() -> Self {
        Self {
            name: "CodexSkillLoader".to_string(),
        }
    }
}

impl Default for CodexSkillLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl FormatLoader for CodexSkillLoader {
    fn can_load(&self, path: &Path) -> bool {
        // 检测是否存在 manifest.yaml 文件
        path.join("manifest.yaml").exists() || path.join("manifest.yml").exists()
    }

    async fn load(&self, path: &Path) -> Result<LoadedSkill, SkillRuntimeError> {
        // 尝试读取 manifest.yaml 或 manifest.yml
        let manifest_path = if path.join("manifest.yaml").exists() {
            path.join("manifest.yaml")
        } else if path.join("manifest.yml").exists() {
            path.join("manifest.yml")
        } else {
            return Err(SkillRuntimeError::InvalidManifest {
                path: path.to_path_buf(),
                message: "No manifest.yaml or manifest.yml found".to_string(),
            });
        };

        // 读取 manifest 文件
        let content = fs::read_to_string(&manifest_path).await.map_err(|e| {
            SkillRuntimeError::LoadFailed {
                path: manifest_path.clone(),
                source: e.into(),
            }
        })?;

        // 解析 YAML
        let metadata = serde_yaml::from_str::<SkillMetadata>(&content).map_err(|e| {
            SkillRuntimeError::InvalidManifest {
                path: manifest_path,
                message: format!("Invalid manifest YAML: {}", e),
            }
        })?;

        // 生成 Skill ID（使用目录名）
        let skill_name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
            SkillRuntimeError::LoadFailed {
                path: path.to_path_buf(),
                source: anyhow::Error::msg("Invalid skill directory name"),
            }
        })?;

        let skill_id = SkillId::from_string(skill_name.to_string()).map_err(|e| {
            SkillRuntimeError::LoadFailed {
                path: path.to_path_buf(),
                source: e,
            }
        })?;

        // 创建 Skill Handler
        let handler = Arc::new(CodexSkillHandler {
            skill_id: skill_id.clone(),
            metadata: metadata.clone(),
            path: path.to_path_buf(),
        });

        Ok(LoadedSkill {
            id: skill_id,
            metadata,
            path: path.to_path_buf(),
            handler,
        })
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Codex Skill 处理器
///
/// 目前只提供基础实现，实际执行逻辑将在后续版本中完善
#[allow(dead_code)]
struct CodexSkillHandler {
    skill_id: SkillId,
    metadata: SkillMetadata,
    path: PathBuf,
}

#[async_trait]
impl SkillHandler for CodexSkillHandler {
    async fn handle(&self, input: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        // 基础实现：返回元数据和输入信息
        Ok(serde_json::json!({
            "skill_id": self.skill_id.as_str(),
            "skill_name": self.metadata.name,
            "version": self.metadata.version,
            "format": "codex",
            "input": input,
            "status": "processed",
            "note": "This is a basic Codex handler implementation."
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_can_load_detects_manifest_yaml() {
        let loader = CodexSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("codex-skill");
        fs::create_dir(&skill_path).await.unwrap();

        // 没有 manifest
        assert!(!loader.can_load(&skill_path));

        // 创建 manifest.yaml
        fs::write(skill_path.join("manifest.yaml"), "test")
            .await
            .unwrap();
        assert!(loader.can_load(&skill_path));
    }

    #[tokio::test]
    async fn test_can_load_detects_manifest_yml() {
        let loader = CodexSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("codex-skill-yml");
        fs::create_dir(&skill_path).await.unwrap();

        // 创建 manifest.yml
        fs::write(skill_path.join("manifest.yml"), "test")
            .await
            .unwrap();
        assert!(loader.can_load(&skill_path));
    }

    #[tokio::test]
    async fn test_load_valid_skill() {
        let loader = CodexSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("codex-test-skill");
        fs::create_dir(&skill_path).await.unwrap();

        let manifest_content = r#"
name: Codex Test Skill
version: 2.0.0
description: A Codex skill for testing
author: Codex Team
homepage: https://codex.example.com
tags:
  - codex
  - test
"#;

        fs::write(skill_path.join("manifest.yaml"), manifest_content)
            .await
            .unwrap();

        let loaded = loader.load(&skill_path).await.unwrap();

        assert_eq!(loaded.metadata.name, "Codex Test Skill");
        assert_eq!(loaded.metadata.version, "2.0.0");
        assert_eq!(loaded.metadata.description, "A Codex skill for testing");
        assert_eq!(loaded.metadata.author, Some("Codex Team".to_string()));
        assert_eq!(
            loaded.metadata.homepage,
            Some("https://codex.example.com".to_string())
        );
        assert_eq!(loaded.metadata.tags, vec!["codex", "test"]);
    }

    #[tokio::test]
    async fn test_load_minimal_skill() {
        let loader = CodexSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("minimal-codex");
        fs::create_dir(&skill_path).await.unwrap();

        let manifest_content = r#"
name: Minimal Codex Skill
version: 1.0.0
description: Minimal Codex skill
"#;

        fs::write(skill_path.join("manifest.yaml"), manifest_content)
            .await
            .unwrap();

        let loaded = loader.load(&skill_path).await.unwrap();

        assert_eq!(loaded.metadata.name, "Minimal Codex Skill");
        assert_eq!(loaded.metadata.version, "1.0.0");
        assert_eq!(loaded.metadata.author, None);
        assert!(loaded.metadata.tags.is_empty());
    }

    #[tokio::test]
    async fn test_load_invalid_yaml() {
        let loader = CodexSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("invalid-codex");
        fs::create_dir(&skill_path).await.unwrap();

        // 无效的 YAML
        let manifest_content = "invalid: yaml: content:";

        fs::write(skill_path.join("manifest.yaml"), manifest_content)
            .await
            .unwrap();

        let result = loader.load(&skill_path).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handler_execution() {
        let loader = CodexSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("exec-codex");
        fs::create_dir(&skill_path).await.unwrap();

        let manifest_content = r#"
name: Execution Test Codex
version: 1.5.0
description: Testing Codex handler
"#;

        fs::write(skill_path.join("manifest.yaml"), manifest_content)
            .await
            .unwrap();

        let loaded = loader.load(&skill_path).await.unwrap();

        // 测试处理器执行
        let input = serde_json::json!({"action": "test"});
        let output = loaded.handler.handle(input.clone()).await.unwrap();

        assert_eq!(output["skill_name"], "Execution Test Codex");
        assert_eq!(output["version"], "1.5.0");
        assert_eq!(output["format"], "codex");
        assert_eq!(output["input"], input);
        assert_eq!(output["status"], "processed");
    }

    #[tokio::test]
    async fn test_load_skill_with_tools() {
        let loader = CodexSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("codex-tools-skill");
        fs::create_dir(&skill_path).await.unwrap();

        let manifest_content = r#"
name: Codex Tools Skill
version: 1.0.0
description: Codex skill with tools
tools:
  - name: lint
    description: Run linter
    parameters_schema:
      type: object
      properties:
        path:
          type: string
    capability_mapping: code.lint
  - name: format
    description: Format code
scripts:
  - scripts/lint.sh
assets:
  - templates/config.yaml
"#;

        fs::write(skill_path.join("manifest.yaml"), manifest_content)
            .await
            .unwrap();

        let loaded = loader.load(&skill_path).await.unwrap();

        assert_eq!(loaded.metadata.tools.len(), 2);
        assert_eq!(loaded.metadata.tools[0].name, "lint");
        assert_eq!(loaded.metadata.tools[0].description, "Run linter");
        assert_eq!(
            loaded.metadata.tools[0].capability_mapping,
            Some("code.lint".to_string())
        );
        assert_eq!(loaded.metadata.tools[1].name, "format");
        assert_eq!(loaded.metadata.tools[1].capability_mapping, None);
        assert_eq!(loaded.metadata.scripts, vec!["scripts/lint.sh"]);
        assert_eq!(loaded.metadata.assets, vec!["templates/config.yaml"]);
    }

    #[tokio::test]
    async fn test_load_prefers_yaml_over_yml() {
        let loader = CodexSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("both-extensions");
        fs::create_dir(&skill_path).await.unwrap();

        // 创建两个文件
        fs::write(
            skill_path.join("manifest.yaml"),
            "name: YAML Version\nversion: 1.0.0\ndescription: From YAML",
        )
        .await
        .unwrap();

        fs::write(
            skill_path.join("manifest.yml"),
            "name: YML Version\nversion: 2.0.0\ndescription: From YML",
        )
        .await
        .unwrap();

        let loaded = loader.load(&skill_path).await.unwrap();

        // 应该优先加载 .yaml
        assert_eq!(loaded.metadata.name, "YAML Version");
        assert_eq!(loaded.metadata.version, "1.0.0");
    }
}
