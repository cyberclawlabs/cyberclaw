//! Superpowers skill 包的工具名翻译。
//!
//! Superpowers 多复用 anthropic 命名（`Bash` / `Read` / `Write`），所以本 translator
//! 实质上是 anthropic alias 的别名层。我们仍然单独提供一个 type，以便：
//!
//! 1. 加载报告里能看到 `superpowers` 桶
//! 2. 后续若 superpowers 演化出独有工具名，只动这一个文件

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
pub struct SuperpowersTranslator;

impl SkillCompat for SuperpowersTranslator {
    fn ecosystem(&self) -> SourceEcosystem {
        SourceEcosystem::Superpowers
    }

    fn translate_tool_name(&self, llm_name: &str) -> Option<String> {
        lookup(ALIASES, llm_name)
    }

    fn known_aliases(&self) -> &[&'static str] {
        &KNOWN_ALIASES
    }
}

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
    fn superpowers_translator_maps_bash() {
        let t = SuperpowersTranslator;
        assert_eq!(t.translate_tool_name("Bash").as_deref(), Some("cmd.run"));
    }

    #[test]
    fn superpowers_translator_ecosystem() {
        let t = SuperpowersTranslator;
        assert_eq!(t.ecosystem(), SourceEcosystem::Superpowers);
    }
}
