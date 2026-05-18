//! Anthropic 官方 claude-skill 的工具名翻译。
//!
//! Anthropic 风格使用 PascalCase 工具名（`Bash`/`Read`/`Write`/`Edit`/`Glob`/`Grep`/
//! `WebFetch`/`WebSearch`），翻译到 cyberclaw facade。

use crate::compat::tool_aliases::lookup;
use crate::compat::{SkillCompat, SourceEcosystem};

const ALIASES: &[(&str, &str)] = &[
    ("Bash", "cmd.run"),
    ("Read", "fs.read"),
    ("Write", "fs.write"),
    ("Edit", "fs.edit"),
    ("MultiEdit", "fs.multiedit"),
    ("WebFetch", "web.fetch"),
    ("WebSearch", "web.search"),
    ("Glob", "search.glob"),
    ("Grep", "search.grep"),
];

#[derive(Debug, Default)]
pub struct AnthropicTranslator;

impl SkillCompat for AnthropicTranslator {
    fn ecosystem(&self) -> SourceEcosystem {
        SourceEcosystem::Anthropic
    }

    fn translate_tool_name(&self, llm_name: &str) -> Option<String> {
        lookup(ALIASES, llm_name)
    }

    fn known_aliases(&self) -> &[&'static str] {
        // 静态字面量数组：每次重新派生开销可忽略；保 trait 签名简单。
        // 通过 leak 避免每次分配 — 见 `KNOWN_ALIASES` 静态。
        &KNOWN_ALIASES
    }
}

/// alias 列表的静态副本（编译期写死，避免运行时分配）。
static KNOWN_ALIASES: [&str; 9] = [
    "Bash",
    "Read",
    "Write",
    "Edit",
    "MultiEdit",
    "WebFetch",
    "WebSearch",
    "Glob",
    "Grep",
];

/// 暴露的 alias 数量。
pub const ALIAS_COUNT: usize = ALIASES.len();

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::tool_aliases::aliases_only;

    #[test]
    fn alias_count_matches_known() {
        // 防止 ALIASES 与 KNOWN_ALIASES 不同步
        assert_eq!(ALIASES.len(), KNOWN_ALIASES.len());
        for (a, _) in ALIASES {
            assert!(
                KNOWN_ALIASES.contains(a),
                "alias {} missing from KNOWN_ALIASES",
                a
            );
        }
        assert_eq!(aliases_only(ALIASES).len(), KNOWN_ALIASES.len());
    }

    #[test]
    fn anthropic_translator_maps_bash() {
        let t = AnthropicTranslator;
        assert_eq!(t.translate_tool_name("Bash").as_deref(), Some("cmd.run"));
    }

    #[test]
    fn anthropic_translator_maps_read_write_edit() {
        let t = AnthropicTranslator;
        assert_eq!(t.translate_tool_name("Read").as_deref(), Some("fs.read"));
        assert_eq!(t.translate_tool_name("Write").as_deref(), Some("fs.write"));
        assert_eq!(t.translate_tool_name("Edit").as_deref(), Some("fs.edit"));
        assert_eq!(
            t.translate_tool_name("MultiEdit").as_deref(),
            Some("fs.multiedit")
        );
    }

    #[test]
    fn anthropic_translator_maps_web_and_search() {
        let t = AnthropicTranslator;
        assert_eq!(
            t.translate_tool_name("WebFetch").as_deref(),
            Some("web.fetch")
        );
        assert_eq!(
            t.translate_tool_name("WebSearch").as_deref(),
            Some("web.search")
        );
        assert_eq!(
            t.translate_tool_name("Glob").as_deref(),
            Some("search.glob")
        );
        assert_eq!(
            t.translate_tool_name("Grep").as_deref(),
            Some("search.grep")
        );
    }

    #[test]
    fn anthropic_translator_returns_none_for_unknown() {
        let t = AnthropicTranslator;
        assert!(t.translate_tool_name("Unknown").is_none());
        // 大小写敏感
        assert!(t.translate_tool_name("bash").is_none());
    }
}
