//! Claude Code Skill 格式加载器
//!
//! Claude Code Skill 格式：
//! - 使用 `SKILL.md` 作为主文件
//! - YAML frontmatter 包含元数据
//! - Markdown 正文包含文档
//! - 可选的 `scripts/` 目录包含可执行脚本

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;

use crate::error::SkillRuntimeError;
use crate::handler::SkillHandler;
use crate::loaders::{FormatLoader, LoadedSkill, SkillMetadata};
use cyberclaw_core::ids::SkillId;

/// Claude Code Skill 加载器
pub struct ClaudeCodeSkillLoader {
    /// 加载器名称
    name: String,
}

impl ClaudeCodeSkillLoader {
    /// 创建新的 Claude Code Skill 加载器
    pub fn new() -> Self {
        Self {
            name: "ClaudeCodeSkillLoader".to_string(),
        }
    }

    /// 解析 SKILL.md 的 frontmatter
    fn parse_frontmatter(&self, content: &str) -> Result<SkillMetadata, SkillRuntimeError> {
        // 查找 frontmatter 分隔符
        let lines: Vec<&str> = content.lines().collect();

        if lines.is_empty() || !lines[0].trim().starts_with("---") {
            return Err(SkillRuntimeError::MetadataParseError(
                "Missing frontmatter delimiter".to_string(),
            ));
        }

        // 查找结束分隔符
        let end_idx = lines
            .iter()
            .skip(1)
            .position(|line| line.trim().starts_with("---"))
            .ok_or_else(|| {
                SkillRuntimeError::MetadataParseError(
                    "Missing frontmatter closing delimiter".to_string(),
                )
            })?
            + 1;

        // 提取 YAML 内容
        let yaml_content = lines[1..end_idx].join("\n");

        // 解析 YAML
        serde_yaml::from_str::<SkillMetadata>(&yaml_content).map_err(|e| {
            SkillRuntimeError::MetadataParseError(format!("Invalid YAML frontmatter: {}", e))
        })
    }
}

impl Default for ClaudeCodeSkillLoader {
    fn default() -> Self {
        Self::new()
    }
}

/// 提取 SKILL.md frontmatter 之后的正文 (markdown body).
///
/// 输入是完整的 SKILL.md 文本; 返回 Some(body) 当能找到两个 `---` 分隔符,
/// 否则 None (let prompt_extension stay empty).
///
/// 文档中"---"之后的全部正文都被保留, 包含表格 / 代码块 / 引用链接 — 这些
/// 才是 LLM 需要看到的 actionable content。
fn extract_body_after_frontmatter(content: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() || !lines[0].trim().starts_with("---") {
        return None;
    }
    let end_idx = lines
        .iter()
        .skip(1)
        .position(|line| line.trim().starts_with("---"))?
        + 1;
    // body 从 closing `---` 之后开始 (跳过空行后剩余全文)
    let body: String = lines
        .iter()
        .skip(end_idx + 1)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    let trimmed = body.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[async_trait]
impl FormatLoader for ClaudeCodeSkillLoader {
    fn can_load(&self, path: &Path) -> bool {
        // 检测是否存在 SKILL.md 文件
        path.join("SKILL.md").exists()
    }

    async fn load(&self, path: &Path) -> Result<LoadedSkill, SkillRuntimeError> {
        let skill_md_path = path.join("SKILL.md");

        // 读取 SKILL.md 文件
        let content = fs::read_to_string(&skill_md_path).await.map_err(|e| {
            SkillRuntimeError::LoadFailed {
                path: skill_md_path.clone(),
                source: e.into(),
            }
        })?;

        // 解析元数据
        let mut metadata = self.parse_frontmatter(&content)?;

        // v1.8 Bug D fix: SKILL.md body (frontmatter 之后的正文) 必须注入 LLM
        // prompt — Anthropic / hermes-agent 都这样做。之前 cb 只保留 frontmatter
        // 导致 LLM 看不到 "Read [pptxgenjs.md] for full details" 等 actionable
        // 内容, 自由发挥写坏 XML。
        // 见 docs/research/skill-md-body-pipeline-gap-2026-05-28.md
        metadata.prompt_body = extract_body_after_frontmatter(&content);

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

        // 创建 Skill Handler（目前使用基础实现）
        let handler = Arc::new(ClaudeCodeSkillHandler {
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

/// Claude Code Skill 处理器
///
/// 目前只提供基础实现，实际执行逻辑将在后续版本中完善
#[allow(dead_code)]
struct ClaudeCodeSkillHandler {
    skill_id: SkillId,
    metadata: SkillMetadata,
    path: PathBuf,
}

#[async_trait]
impl SkillHandler for ClaudeCodeSkillHandler {
    async fn handle(&self, input: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        // 基础实现：返回元数据和输入信息
        Ok(serde_json::json!({
            "skill_id": self.skill_id.as_str(),
            "skill_name": self.metadata.name,
            "version": self.metadata.version,
            "input": input,
            "status": "processed",
            "note": "This is a basic handler implementation. Full execution logic will be added in future versions."
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_can_load_detects_skill_md() {
        let loader = ClaudeCodeSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("test-skill");
        fs::create_dir(&skill_path).await.unwrap();

        // 没有 SKILL.md
        assert!(!loader.can_load(&skill_path));

        // 创建 SKILL.md
        fs::write(skill_path.join("SKILL.md"), "test")
            .await
            .unwrap();
        assert!(loader.can_load(&skill_path));
    }

    #[tokio::test]
    async fn test_load_valid_skill() {
        let loader = ClaudeCodeSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("test-skill");
        fs::create_dir(&skill_path).await.unwrap();

        let skill_content = r#"---
name: Test Skill
version: 1.0.0
description: A test skill for unit testing
author: Test Author
tags: [test, example]
---

# Test Skill

This is a test skill.
"#;

        fs::write(skill_path.join("SKILL.md"), skill_content)
            .await
            .unwrap();

        let loaded = loader.load(&skill_path).await.unwrap();

        assert_eq!(loaded.metadata.name, "Test Skill");
        assert_eq!(loaded.metadata.version, "1.0.0");
        assert_eq!(loaded.metadata.description, "A test skill for unit testing");
        assert_eq!(loaded.metadata.author, Some("Test Author".to_string()));
        assert_eq!(loaded.metadata.tags, vec!["test", "example"]);
        assert_eq!(loaded.id.as_str(), "test-skill");
    }

    #[tokio::test]
    async fn test_load_skill_without_optional_fields() {
        let loader = ClaudeCodeSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("minimal-skill");
        fs::create_dir(&skill_path).await.unwrap();

        let skill_content = r#"---
name: Minimal Skill
version: 0.1.0
description: Minimal skill without optional fields
---

# Minimal Skill
"#;

        fs::write(skill_path.join("SKILL.md"), skill_content)
            .await
            .unwrap();

        let loaded = loader.load(&skill_path).await.unwrap();

        assert_eq!(loaded.metadata.name, "Minimal Skill");
        assert_eq!(loaded.metadata.author, None);
        assert_eq!(loaded.metadata.homepage, None);
        assert!(loaded.metadata.tags.is_empty());
    }

    #[tokio::test]
    async fn test_load_skill_invalid_frontmatter() {
        let loader = ClaudeCodeSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("invalid-skill");
        fs::create_dir(&skill_path).await.unwrap();

        // 缺少结束分隔符
        let skill_content = r#"---
name: Invalid Skill
version: 1.0.0

# Invalid Skill
"#;

        fs::write(skill_path.join("SKILL.md"), skill_content)
            .await
            .unwrap();

        let result = loader.load(&skill_path).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_load_skill_missing_required_fields() {
        let loader = ClaudeCodeSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("incomplete-skill");
        fs::create_dir(&skill_path).await.unwrap();

        // 缺少必需字段
        let skill_content = r#"---
name: Incomplete Skill
---

# Incomplete Skill
"#;

        fs::write(skill_path.join("SKILL.md"), skill_content)
            .await
            .unwrap();

        let result = loader.load(&skill_path).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handler_execution() {
        let loader = ClaudeCodeSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("exec-test-skill");
        fs::create_dir(&skill_path).await.unwrap();

        let skill_content = r#"---
name: Execution Test
version: 1.0.0
description: Test skill execution
---

# Execution Test
"#;

        fs::write(skill_path.join("SKILL.md"), skill_content)
            .await
            .unwrap();

        let loaded = loader.load(&skill_path).await.unwrap();

        // 测试处理器执行
        let input = serde_json::json!({"test": "data"});
        let output = loaded.handler.handle(input.clone()).await.unwrap();

        assert_eq!(output["skill_name"], "Execution Test");
        assert_eq!(output["version"], "1.0.0");
        assert_eq!(output["input"], input);
        assert_eq!(output["status"], "processed");
    }

    #[tokio::test]
    async fn test_load_skill_with_tools() {
        let loader = ClaudeCodeSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("tools-skill");
        fs::create_dir(&skill_path).await.unwrap();

        let skill_content = r#"---
name: Tools Skill
version: 1.0.0
description: Skill with tool declarations
tools:
  - name: search
    description: Search for content
    parameters_schema:
      type: object
      properties:
        query:
          type: string
    capability_mapping: search.web
  - name: fetch
    description: Fetch a URL
scripts:
  - scripts/setup.sh
  - scripts/run.py
assets:
  - data/config.json
---

# Tools Skill
"#;

        fs::write(skill_path.join("SKILL.md"), skill_content)
            .await
            .unwrap();

        let loaded = loader.load(&skill_path).await.unwrap();

        assert_eq!(loaded.metadata.tools.len(), 2);
        assert_eq!(loaded.metadata.tools[0].name, "search");
        assert_eq!(loaded.metadata.tools[0].description, "Search for content");
        assert_eq!(
            loaded.metadata.tools[0].capability_mapping,
            Some("search.web".to_string())
        );
        assert_eq!(loaded.metadata.tools[1].name, "fetch");
        assert_eq!(loaded.metadata.tools[1].description, "Fetch a URL");
        assert_eq!(loaded.metadata.tools[1].capability_mapping, None);
        assert_eq!(
            loaded.metadata.scripts,
            vec!["scripts/setup.sh", "scripts/run.py"]
        );
        assert_eq!(loaded.metadata.assets, vec!["data/config.json"]);
    }

    #[tokio::test]
    async fn test_load_skill_with_hermes_extensions() {
        let loader = ClaudeCodeSkillLoader::new();
        let temp_dir = TempDir::new().unwrap();
        let skill_path = temp_dir.path().join("hermes-skill");
        fs::create_dir(&skill_path).await.unwrap();

        let skill_content = r#"---
name: Hermes Skill
version: 1.0.0
description: Skill with Hermes extensions
platforms:
  - linux
  - macos
triggers:
  - on_commit
  - on_push
required_toolsets:
  - git
  - docker
tools:
  - name: deploy
    description: Deploy to platform
---

# Hermes Skill
"#;

        fs::write(skill_path.join("SKILL.md"), skill_content)
            .await
            .unwrap();

        let loaded = loader.load(&skill_path).await.unwrap();

        assert_eq!(loaded.metadata.platforms, vec!["linux", "macos"]);
        assert_eq!(loaded.metadata.triggers, vec!["on_commit", "on_push"]);
        assert_eq!(loaded.metadata.required_toolsets, vec!["git", "docker"]);
        assert_eq!(loaded.metadata.tools.len(), 1);
        assert_eq!(loaded.metadata.tools[0].name, "deploy");
    }

    #[tokio::test]
    async fn test_parse_frontmatter_with_complex_tags() {
        let loader = ClaudeCodeSkillLoader::new();

        let content = r#"---
name: Complex Skill
version: 2.1.0
description: Skill with complex metadata
author: Complex Author
homepage: https://example.com
tags:
  - data-processing
  - automation
  - productivity
---

# Complex Skill
"#;

        let metadata = loader.parse_frontmatter(content).unwrap();

        assert_eq!(metadata.name, "Complex Skill");
        assert_eq!(metadata.version, "2.1.0");
        assert_eq!(
            metadata.tags,
            vec!["data-processing", "automation", "productivity"]
        );
        assert_eq!(metadata.homepage, Some("https://example.com".to_string()));
    }
}
