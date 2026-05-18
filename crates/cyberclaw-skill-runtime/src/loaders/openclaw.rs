//! OpenClaw Skill 格式加载器
//!
//! OpenClaw Skill 格式：
//! - 使用 `skill.toml` 作为元数据文件
//! - 可选的 `README.md` 包含文档
//! - 可选的 `handlers/` 目录包含处理器实现

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;

use crate::error::SkillRuntimeError;
use crate::handler::SkillHandler;
use crate::loaders::{FormatLoader, LoadedSkill, SkillMetadata, SkillToolDeclaration};
use cyberclaw_core::ids::SkillId;

/// OpenClaw Skill 的 TOML 格式元数据
#[derive(Debug, Clone, serde::Deserialize)]
struct OpenClawManifest {
    skill: OpenClawSkillInfo,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct OpenClawSkillInfo {
    name: String,
    version: String,
    description: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    tools: Vec<OpenClawToolDecl>,
    #[serde(default)]
    scripts: Vec<String>,
    #[serde(default)]
    assets: Vec<String>,
}

/// OpenClaw 工具声明 (TOML 格式)
#[derive(Debug, Clone, serde::Deserialize)]
struct OpenClawToolDecl {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    parameters_schema: serde_json::Value,
    #[serde(default)]
    capability_mapping: Option<String>,
}

/// OpenClaw Skill 加载器
pub struct OpenClawSkillLoader {
    /// 加载器名称
    name: String,
}

impl OpenClawSkillLoader {
    /// 创建新的 OpenClaw Skill 加载器
    pub fn new() -> Self {
        Self {
            name: "OpenClawSkillLoader".to_string(),
        }
    }

    /// 将 OpenClaw 格式转换为统一的 SkillMetadata
    fn convert_metadata(&self, manifest: OpenClawManifest) -> SkillMetadata {
        let tools = manifest
            .skill
            .tools
            .into_iter()
            .map(|t| SkillToolDeclaration {
                name: t.name,
                description: t.description,
                parameters_schema: t.parameters_schema,
                capability_mapping: t.capability_mapping,
            })
            .collect();

        SkillMetadata {
            name: manifest.skill.name,
            version: manifest.skill.version,
            description: manifest.skill.description,
            author: manifest.skill.author,
            homepage: manifest.skill.homepage,
            tags: manifest.skill.tags,
            tools,
            scripts: manifest.skill.scripts,
            assets: manifest.skill.assets,
            platforms: Vec::new(),
            triggers: Vec::new(),
            required_toolsets: Vec::new(),
            source_ecosystem: None,
            allowed_capabilities: Vec::new(),
        }
    }
}

impl Default for OpenClawSkillLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl FormatLoader for OpenClawSkillLoader {
    fn can_load(&self, path: &Path) -> bool {
        // 检测是否存在 skill.toml 文件
        path.join("skill.toml").exists()
    }

    async fn load(&self, path: &Path) -> Result<LoadedSkill, SkillRuntimeError> {
        let toml_path = path.join("skill.toml");

        // 读取 skill.toml 文件
        let content =
            fs::read_to_string(&toml_path)
                .await
                .map_err(|e| SkillRuntimeError::LoadFailed {
                    path: toml_path.clone(),
                    source: e.into(),
                })?;

        // 解析 TOML
        let manifest = toml::from_str::<OpenClawManifest>(&content).map_err(|e| {
            SkillRuntimeError::InvalidManifest {
                path: toml_path,
                message: format!("Invalid TOML format: {}", e),
            }
        })?;

        // 转换元数据
        let metadata = self.convert_metadata(manifest);

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
        let handler = Arc::new(OpenClawSkillHandler {
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

/// OpenClaw Skill 处理器
///
/// 目前只提供基础实现，实际执行逻辑将在后续版本中完善
#[allow(dead_code)]
struct OpenClawSkillHandler {
    skill_id: SkillId,
    metadata: SkillMetadata,
    path: PathBuf,
}

#[async_trait]
impl SkillHandler for OpenClawSkillHandler {
    async fn handle(&self, input: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        // 基础实现：返回元数据和输入信息
        Ok(serde_json::json!({
            "skill_id": self.skill_id.as_str(),
            "skill_name": self.metadata.name,
            "version": self.metadata.version,
            "format": "openclaw",
            "input": input,
            "status": "processed",
            "note": "This is a basic OpenClaw handler implementation."
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_can_load_detects_skill_toml() {
        let loader = OpenClawSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("openclaw-skill");
        fs::create_dir(&skill_path).await.unwrap();

        // 没有 skill.toml
        assert!(!loader.can_load(&skill_path));

        // 创建 skill.toml
        fs::write(skill_path.join("skill.toml"), "test")
            .await
            .unwrap();
        assert!(loader.can_load(&skill_path));
    }

    #[tokio::test]
    async fn test_load_valid_skill() {
        let loader = OpenClawSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("openclaw-test");
        fs::create_dir(&skill_path).await.unwrap();

        let toml_content = r#"
[skill]
name = "OpenClaw Test Skill"
version = "3.0.0"
description = "An OpenClaw skill for testing"
author = "OpenClaw Team"
homepage = "https://openclaw.example.com"
tags = ["openclaw", "test", "example"]
"#;

        fs::write(skill_path.join("skill.toml"), toml_content)
            .await
            .unwrap();

        let loaded = loader.load(&skill_path).await.unwrap();

        assert_eq!(loaded.metadata.name, "OpenClaw Test Skill");
        assert_eq!(loaded.metadata.version, "3.0.0");
        assert_eq!(loaded.metadata.description, "An OpenClaw skill for testing");
        assert_eq!(loaded.metadata.author, Some("OpenClaw Team".to_string()));
        assert_eq!(
            loaded.metadata.homepage,
            Some("https://openclaw.example.com".to_string())
        );
        assert_eq!(loaded.metadata.tags, vec!["openclaw", "test", "example"]);
    }

    #[tokio::test]
    async fn test_load_minimal_skill() {
        let loader = OpenClawSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("minimal-openclaw");
        fs::create_dir(&skill_path).await.unwrap();

        let toml_content = r#"
[skill]
name = "Minimal OpenClaw"
version = "1.0.0"
description = "Minimal OpenClaw skill"
"#;

        fs::write(skill_path.join("skill.toml"), toml_content)
            .await
            .unwrap();

        let loaded = loader.load(&skill_path).await.unwrap();

        assert_eq!(loaded.metadata.name, "Minimal OpenClaw");
        assert_eq!(loaded.metadata.version, "1.0.0");
        assert_eq!(loaded.metadata.author, None);
        assert!(loaded.metadata.tags.is_empty());
    }

    #[tokio::test]
    async fn test_load_invalid_toml() {
        let loader = OpenClawSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("invalid-openclaw");
        fs::create_dir(&skill_path).await.unwrap();

        // 无效的 TOML
        let toml_content = "[skill\nname = invalid";

        fs::write(skill_path.join("skill.toml"), toml_content)
            .await
            .unwrap();

        let result = loader.load(&skill_path).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_load_missing_required_section() {
        let loader = OpenClawSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("incomplete-openclaw");
        fs::create_dir(&skill_path).await.unwrap();

        // 缺少 [skill] section
        let toml_content = r#"
name = "Wrong Structure"
version = "1.0.0"
"#;

        fs::write(skill_path.join("skill.toml"), toml_content)
            .await
            .unwrap();

        let result = loader.load(&skill_path).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handler_execution() {
        let loader = OpenClawSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("exec-openclaw");
        fs::create_dir(&skill_path).await.unwrap();

        let toml_content = r#"
[skill]
name = "Execution Test OpenClaw"
version = "2.5.0"
description = "Testing OpenClaw handler"
"#;

        fs::write(skill_path.join("skill.toml"), toml_content)
            .await
            .unwrap();

        let loaded = loader.load(&skill_path).await.unwrap();

        // 测试处理器执行
        let input = serde_json::json!({"operation": "test"});
        let output = loaded.handler.handle(input.clone()).await.unwrap();

        assert_eq!(output["skill_name"], "Execution Test OpenClaw");
        assert_eq!(output["version"], "2.5.0");
        assert_eq!(output["format"], "openclaw");
        assert_eq!(output["input"], input);
        assert_eq!(output["status"], "processed");
    }

    #[tokio::test]
    async fn test_load_skill_with_tools() {
        let loader = OpenClawSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("openclaw-tools");
        fs::create_dir(&skill_path).await.unwrap();

        let toml_content = r#"
[skill]
name = "OpenClaw Tools Skill"
version = "1.0.0"
description = "OpenClaw skill with tools"
scripts = ["scripts/init.sh"]
assets = ["data/schema.json"]

[[skill.tools]]
name = "analyze"
description = "Analyze code"
capability_mapping = "code.analyze"

[[skill.tools]]
name = "report"
description = "Generate report"
"#;

        fs::write(skill_path.join("skill.toml"), toml_content)
            .await
            .unwrap();

        let loaded = loader.load(&skill_path).await.unwrap();

        assert_eq!(loaded.metadata.tools.len(), 2);
        assert_eq!(loaded.metadata.tools[0].name, "analyze");
        assert_eq!(loaded.metadata.tools[0].description, "Analyze code");
        assert_eq!(
            loaded.metadata.tools[0].capability_mapping,
            Some("code.analyze".to_string())
        );
        assert_eq!(loaded.metadata.tools[1].name, "report");
        assert_eq!(loaded.metadata.tools[1].capability_mapping, None);
        assert_eq!(loaded.metadata.scripts, vec!["scripts/init.sh"]);
        assert_eq!(loaded.metadata.assets, vec!["data/schema.json"]);
    }

    #[tokio::test]
    async fn test_convert_metadata_with_all_fields() {
        let loader = OpenClawSkillLoader::new();

        let manifest = OpenClawManifest {
            skill: OpenClawSkillInfo {
                name: "Full Metadata Skill".to_string(),
                version: "1.2.3".to_string(),
                description: "Complete metadata example".to_string(),
                author: Some("Author Name".to_string()),
                homepage: Some("https://example.com".to_string()),
                tags: vec!["tag1".to_string(), "tag2".to_string(), "tag3".to_string()],
                tools: Vec::new(),
                scripts: Vec::new(),
                assets: Vec::new(),
            },
        };

        let metadata = loader.convert_metadata(manifest);

        assert_eq!(metadata.name, "Full Metadata Skill");
        assert_eq!(metadata.version, "1.2.3");
        assert_eq!(metadata.description, "Complete metadata example");
        assert_eq!(metadata.author, Some("Author Name".to_string()));
        assert_eq!(metadata.homepage, Some("https://example.com".to_string()));
        assert_eq!(metadata.tags, vec!["tag1", "tag2", "tag3"]);
    }
}
