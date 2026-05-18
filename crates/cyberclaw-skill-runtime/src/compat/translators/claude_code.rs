//! Claude Code skill 的工具名翻译。
//!
//! Claude Code skill 通常引用 anthropic 系列工具名（`Bash`/`Read`/`Write`），但也
//! 可能掺入小写形式（`bash`/`read`）。这里覆盖大小写两套别名。

use crate::compat::tool_aliases::lookup;
use crate::compat::{SkillCompat, SourceEcosystem};

const ALIASES: &[(&str, &str)] = &[
    // 与 anthropic 一致的大写
    ("Bash", "cmd.run"),
    ("Read", "fs.read"),
    ("Write", "fs.write"),
    ("Edit", "fs.edit"),
    ("MultiEdit", "fs.multiedit"),
    ("WebFetch", "web.fetch"),
    ("WebSearch", "web.search"),
    ("Glob", "search.glob"),
    ("Grep", "search.grep"),
    // Claude Code 命令行风格
    ("ls_dir", "search.glob"),
    ("execute_bash", "cmd.run"),
];

#[derive(Debug, Default)]
pub struct ClaudeCodeTranslator;

impl SkillCompat for ClaudeCodeTranslator {
    fn ecosystem(&self) -> SourceEcosystem {
        SourceEcosystem::ClaudeCode
    }

    fn translate_tool_name(&self, llm_name: &str) -> Option<String> {
        lookup(ALIASES, llm_name)
    }

    fn known_aliases(&self) -> &[&'static str] {
        &KNOWN_ALIASES
    }
}

static KNOWN_ALIASES: [&str; 11] = [
    "Bash",
    "Read",
    "Write",
    "Edit",
    "MultiEdit",
    "WebFetch",
    "WebSearch",
    "Glob",
    "Grep",
    "ls_dir",
    "execute_bash",
];

pub const ALIAS_COUNT: usize = ALIASES.len();

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::tool_aliases::aliases_only;

    #[test]
    fn alias_count_matches_known() {
        assert_eq!(ALIASES.len(), KNOWN_ALIASES.len());
        assert_eq!(aliases_only(ALIASES).len(), KNOWN_ALIASES.len());
    }

    #[test]
    fn claude_code_translator_maps_bash() {
        let t = ClaudeCodeTranslator;
        assert_eq!(t.translate_tool_name("Bash").as_deref(), Some("cmd.run"));
        assert_eq!(
            t.translate_tool_name("execute_bash").as_deref(),
            Some("cmd.run")
        );
    }

    #[test]
    fn claude_code_translator_returns_none_for_unknown() {
        let t = ClaudeCodeTranslator;
        assert!(t.translate_tool_name("nope").is_none());
    }

    #[test]
    fn claude_code_translator_ecosystem() {
        let t = ClaudeCodeTranslator;
        assert_eq!(t.ecosystem(), SourceEcosystem::ClaudeCode);
    }
}
