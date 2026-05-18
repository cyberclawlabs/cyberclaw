// tests/helpers/skill.rs
// Skill 测试辅助工具

use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tokio::fs;

/// 创建 Claude Code Skill
pub async fn create_claude_code_skill(name: &str) -> Result<(TempDir, PathBuf)> {
    let temp_dir = TempDir::new()?;
    let skill_dir = temp_dir.path().join(name);
    fs::create_dir_all(&skill_dir).await?;

    // 创建 SKILL.md
    let skill_md = format!(
        r#"---
name: {}
version: 1.0.0
description: Test Claude Code skill
author: Test Author
tags: [test, claude]
---

# {} Skill

This is a test skill for Claude Code.

## Usage

This skill provides the following capabilities:

- `{}.test`: Run a test command
- `{}.echo`: Echo back the input

## Examples

```bash
# Run test
cyberclaw skill run {}.test

# Echo message
cyberclaw skill run {}.echo --message "Hello"
```

## Configuration

No configuration required.
"#,
        name, name, name, name, name, name
    );

    fs::write(skill_dir.join("SKILL.md"), skill_md).await?;

    // 创建 scripts 目录
    let scripts_dir = skill_dir.join("scripts");
    fs::create_dir_all(&scripts_dir).await?;

    // 创建示例脚本
    let test_script = r#"#!/bin/bash
echo "Running test for Claude Code skill"
exit 0
"#;

    fs::write(scripts_dir.join("test.sh"), test_script).await?;

    let echo_script = r#"#!/bin/bash
MESSAGE="${1:-Hello World}"
echo "$MESSAGE"
"#;

    fs::write(scripts_dir.join("echo.sh"), echo_script).await?;

    // 创建 references 目录
    let refs_dir = skill_dir.join("references");
    fs::create_dir_all(&refs_dir).await?;

    // 创建示例参考文档
    let reference = r#"# Reference Documentation

This is a reference document for the skill.
"#;

    fs::write(refs_dir.join("README.md"), reference).await?;

    Ok((temp_dir, skill_dir))
}

/// 创建 Codex Skill
pub async fn create_codex_skill(name: &str) -> Result<(TempDir, PathBuf)> {
    let temp_dir = TempDir::new()?;
    let skill_dir = temp_dir.path().join(name);
    fs::create_dir_all(&skill_dir).await?;

    // 创建 skill.json (Codex 格式)
    let skill_json = json!({
        "name": name,
        "version": "1.0.0",
        "description": "Test Codex skill",
        "author": "Test Author",
        "capabilities": [
            {
                "id": format!("{}.process", name),
                "description": "Process data",
                "input": {
                    "type": "object",
                    "properties": {
                        "data": {"type": "string"}
                    }
                },
                "output": {
                    "type": "object",
                    "properties": {
                        "result": {"type": "string"}
                    }
                }
            }
        ],
        "dependencies": [],
        "scripts": {
            "process": "scripts/process.py"
        }
    });

    fs::write(
        skill_dir.join("skill.json"),
        serde_json::to_string_pretty(&skill_json)?,
    )
    .await?;

    // 创建 scripts 目录
    let scripts_dir = skill_dir.join("scripts");
    fs::create_dir_all(&scripts_dir).await?;

    // 创建 Python 脚本
    let process_script = r#"#!/usr/bin/env python3
import json
import sys

def process(data):
    return {"result": f"Processed: {data}"}

if __name__ == "__main__":
    input_data = json.loads(sys.argv[1])
    result = process(input_data.get("data", ""))
    print(json.dumps(result))
"#;

    fs::write(scripts_dir.join("process.py"), process_script).await?;

    Ok((temp_dir, skill_dir))
}

/// 创建 OpenClaw Skill
pub async fn create_openclaw_skill(name: &str) -> Result<(TempDir, PathBuf)> {
    let temp_dir = TempDir::new()?;
    let skill_dir = temp_dir.path().join(name);
    fs::create_dir_all(&skill_dir).await?;

    // 创建 manifest.yaml (OpenClaw 格式)
    let manifest_yaml = format!(
        r#"apiVersion: v1
kind: Skill
metadata:
  name: {}
  version: 1.0.0
  author: Test Author
  description: Test OpenClaw skill

spec:
  capabilities:
    - id: {}.analyze
      description: Analyze input data
      type: command
      command: scripts/analyze.sh

    - id: {}.transform
      description: Transform data format
      type: function
      runtime: node
      handler: scripts/transform.js

  resources:
    - type: config
      path: config/
    - type: template
      path: templates/

  requirements:
    - runtime: bash
      version: ">=4.0"
    - runtime: node
      version: ">=14.0"
"#,
        name, name, name
    );

    fs::write(skill_dir.join("manifest.yaml"), manifest_yaml).await?;

    // 创建 scripts 目录
    let scripts_dir = skill_dir.join("scripts");
    fs::create_dir_all(&scripts_dir).await?;

    // 创建 bash 脚本
    let analyze_script = r#"#!/bin/bash
INPUT="$1"
echo "Analyzing: $INPUT"
echo "Analysis complete"
"#;

    fs::write(scripts_dir.join("analyze.sh"), analyze_script).await?;

    // 创建 Node.js 脚本
    let transform_script = r#"module.exports = function(input) {
    return {
        transformed: input.toUpperCase(),
        timestamp: new Date().toISOString()
    };
};
"#;

    fs::write(scripts_dir.join("transform.js"), transform_script).await?;

    // 创建 config 目录
    let config_dir = skill_dir.join("config");
    fs::create_dir_all(&config_dir).await?;

    let config = r#"# Configuration
debug: false
timeout: 30
"#;

    fs::write(config_dir.join("default.yaml"), config).await?;

    Ok((temp_dir, skill_dir))
}

/// Skill 测试沙箱
pub struct SkillTestSandbox {
    work_dir: TempDir,
    skills: HashMap<String, SkillInfo>,
}

#[derive(Clone, Debug)]
pub struct SkillInfo {
    pub path: PathBuf,
    pub format: SkillFormat,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SkillFormat {
    ClaudeCode,
    Codex,
    OpenClaw,
}

impl SkillTestSandbox {
    pub async fn new() -> Result<Self> {
        let work_dir = TempDir::new()?;

        // 创建技能目录
        let skills_dir = work_dir.path().join("skills");
        fs::create_dir_all(&skills_dir).await?;

        Ok(Self {
            work_dir,
            skills: HashMap::new(),
        })
    }

    /// 添加 Claude Code Skill
    pub async fn add_claude_skill(&mut self, name: &str) -> Result<PathBuf> {
        let skill_dir = self.work_dir.path().join("skills").join(name);
        fs::create_dir_all(&skill_dir).await?;

        // 创建简单的 SKILL.md
        let skill_md = format!(
            r#"---
name: {}
version: 1.0.0
---

# {} Skill
"#,
            name, name
        );

        fs::write(skill_dir.join("SKILL.md"), skill_md).await?;

        self.skills.insert(
            name.to_string(),
            SkillInfo {
                path: skill_dir.clone(),
                format: SkillFormat::ClaudeCode,
                capabilities: vec![format!("{}.test", name)],
            },
        );

        Ok(skill_dir)
    }

    /// 添加 Codex Skill
    pub async fn add_codex_skill(&mut self, name: &str) -> Result<PathBuf> {
        let skill_dir = self.work_dir.path().join("skills").join(name);
        fs::create_dir_all(&skill_dir).await?;

        let skill_json = json!({
            "name": name,
            "version": "1.0.0",
            "capabilities": []
        });

        fs::write(
            skill_dir.join("skill.json"),
            serde_json::to_string(&skill_json)?,
        )
        .await?;

        self.skills.insert(
            name.to_string(),
            SkillInfo {
                path: skill_dir.clone(),
                format: SkillFormat::Codex,
                capabilities: vec![],
            },
        );

        Ok(skill_dir)
    }

    /// 添加 OpenClaw Skill
    pub async fn add_openclaw_skill(&mut self, name: &str) -> Result<PathBuf> {
        let skill_dir = self.work_dir.path().join("skills").join(name);
        fs::create_dir_all(&skill_dir).await?;

        let manifest = format!(
            r#"apiVersion: v1
kind: Skill
metadata:
  name: {}
"#,
            name
        );

        fs::write(skill_dir.join("manifest.yaml"), manifest).await?;

        self.skills.insert(
            name.to_string(),
            SkillInfo {
                path: skill_dir.clone(),
                format: SkillFormat::OpenClaw,
                capabilities: vec![],
            },
        );

        Ok(skill_dir)
    }

    pub fn skills_dir(&self) -> PathBuf {
        self.work_dir.path().join("skills")
    }

    pub fn get_skill(&self, name: &str) -> Option<&SkillInfo> {
        self.skills.get(name)
    }

    pub fn list_skills(&self) -> Vec<String> {
        self.skills.keys().cloned().collect()
    }
}

/// Mock Skill 执行器
pub struct MockSkillExecutor {
    execution_log: Arc<tokio::sync::Mutex<Vec<SkillExecution>>>,
}

#[derive(Clone, Debug)]
pub struct SkillExecution {
    pub skill_name: String,
    pub capability: String,
    pub params: Value,
    pub result: Value,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl MockSkillExecutor {
    pub fn new() -> Self {
        Self {
            execution_log: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }

    pub async fn execute(
        &self,
        skill_name: &str,
        capability: &str,
        params: Value,
    ) -> Result<Value> {
        // 模拟执行
        let result = json!({
            "status": "success",
            "output": format!("Executed {}.{}", skill_name, capability),
            "params": params,
        });

        // 记录执行
        let execution = SkillExecution {
            skill_name: skill_name.to_string(),
            capability: capability.to_string(),
            params: params.clone(),
            result: result.clone(),
            timestamp: chrono::Utc::now(),
        };

        self.execution_log.lock().await.push(execution);

        Ok(result)
    }

    pub async fn get_execution_log(&self) -> Vec<SkillExecution> {
        self.execution_log.lock().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_claude_skill() {
        let (_temp, skill_dir) = create_claude_code_skill("test-claude")
            .await
            .unwrap();

        assert!(skill_dir.join("SKILL.md").exists());
        assert!(skill_dir.join("scripts").exists());
        assert!(skill_dir.join("references").exists());
    }

    #[tokio::test]
    async fn test_create_codex_skill() {
        let (_temp, skill_dir) = create_codex_skill("test-codex").await.unwrap();

        assert!(skill_dir.join("skill.json").exists());
        assert!(skill_dir.join("scripts").exists());
    }

    #[tokio::test]
    async fn test_create_openclaw_skill() {
        let (_temp, skill_dir) = create_openclaw_skill("test-openclaw")
            .await
            .unwrap();

        assert!(skill_dir.join("manifest.yaml").exists());
        assert!(skill_dir.join("scripts").exists());
        assert!(skill_dir.join("config").exists());
    }

    #[tokio::test]
    async fn test_skill_sandbox() {
        let mut sandbox = SkillTestSandbox::new().await.unwrap();

        sandbox.add_claude_skill("claude-test").await.unwrap();
        sandbox.add_codex_skill("codex-test").await.unwrap();
        sandbox.add_openclaw_skill("openclaw-test").await.unwrap();

        let skills = sandbox.list_skills();
        assert_eq!(skills.len(), 3);

        let claude_skill = sandbox.get_skill("claude-test").unwrap();
        assert_eq!(claude_skill.format, SkillFormat::ClaudeCode);
    }
}