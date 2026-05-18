//! Skill context types for the declarative skill model.
//!
//! In CyberClaw's architecture, Skills provide context (prompts, tool declarations,
//! references) but do NOT directly execute capabilities. Execution always flows
//! through Connector -> Capability. These types represent the context a skill
//! contributes to the agentic loop.

use crate::loaders::{SkillMetadata, SkillToolDeclaration};

/// Context provided by a skill for injection into the agentic loop.
///
/// Instead of invoking a skill directly, consumers should retrieve the skill's
/// context and feed it into the Connector -> Capability execution path.
#[derive(Debug, Clone)]
pub struct SkillContext {
    /// Skill metadata (name, version, description, etc.)
    pub metadata: SkillMetadata,
    /// System prompt extension from this skill.
    pub prompt_extension: String,
    /// Tool definitions this skill declares (each maps to a Capability via Connector).
    pub tool_declarations: Vec<ToolDeclaration>,
    /// Reference documents/files this skill provides.
    pub references: Vec<SkillReference>,
    /// Script files bundled with the skill.
    pub scripts: Vec<String>,
    /// Asset files bundled with the skill.
    pub assets: Vec<String>,
    /// Origin format identifier (e.g. "claude-code", "codex", "openclaw", "hermes").
    pub origin_format: String,
}

/// A tool declared by a skill (maps to a Capability via Connector).
///
/// Unified type alias for `SkillToolDeclaration` — skills declare tools they
/// need but do not execute them directly. The platform resolves each declaration
/// to a concrete Connector + Capability at execution time.
pub type ToolDeclaration = SkillToolDeclaration;

/// A reference document provided by a skill.
///
/// Skills can bundle markdown files, code snippets, or other reference material
/// that agents may consult during execution.
#[derive(Debug, Clone)]
pub struct SkillReference {
    /// Reference name/title.
    pub name: String,
    /// Content (markdown, text, etc.)
    pub content: String,
    /// MIME type (e.g. "text/markdown", "text/plain").
    pub content_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_context_creation() {
        let ctx = SkillContext {
            metadata: SkillMetadata {
                name: "test-skill".to_string(),
                version: "1.0.0".to_string(),
                description: "A test skill".to_string(),
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
            },
            prompt_extension: "You are a helpful assistant.".to_string(),
            tool_declarations: vec![ToolDeclaration {
                name: "run-command".to_string(),
                description: "Execute a shell command".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" }
                    }
                }),
                capability_mapping: Some("local-shell:exec".to_string()),
            }],
            references: vec![SkillReference {
                name: "usage-guide".to_string(),
                content: "# Usage Guide\nRun commands safely.".to_string(),
                content_type: "text/markdown".to_string(),
            }],
            scripts: vec![],
            assets: vec![],
            origin_format: "openclaw".to_string(),
        };

        assert_eq!(ctx.metadata.name, "test-skill");
        assert_eq!(ctx.tool_declarations.len(), 1);
        assert_eq!(ctx.tool_declarations[0].name, "run-command");
        assert_eq!(
            ctx.tool_declarations[0].capability_mapping.as_deref(),
            Some("local-shell:exec")
        );
        assert_eq!(ctx.references.len(), 1);
        assert_eq!(ctx.references[0].content_type, "text/markdown");
    }

    #[test]
    fn test_tool_declaration_without_mapping() {
        let decl = ToolDeclaration {
            name: "search".to_string(),
            description: "Search documents".to_string(),
            parameters_schema: serde_json::json!({}),
            capability_mapping: None,
        };

        assert!(decl.capability_mapping.is_none());
    }

    #[test]
    fn test_skill_context_clone() {
        let ctx = SkillContext {
            metadata: SkillMetadata {
                name: "clone-test".to_string(),
                version: "0.1.0".to_string(),
                description: "Clone test".to_string(),
                author: Some("tester".to_string()),
                homepage: None,
                tags: vec!["test".to_string()],
                tools: vec![],
                scripts: vec![],
                assets: vec![],
                platforms: vec![],
                triggers: vec![],
                required_toolsets: vec![],
                source_ecosystem: None,
                allowed_capabilities: vec![],
            },
            prompt_extension: String::new(),
            tool_declarations: vec![],
            references: vec![],
            scripts: vec![],
            assets: vec![],
            origin_format: String::new(),
        };

        let cloned = ctx.clone();
        assert_eq!(cloned.metadata.name, ctx.metadata.name);
        assert_eq!(cloned.metadata.author, ctx.metadata.author);
    }
}
