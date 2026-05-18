//! OpenClaw skill 的工具名翻译。
//!
//! OpenClaw 多用 snake_case 命名 + 自定义 CLI 工具（如 `obsidian_cli`），主流子集
//! 与 hermes 重叠（`fs_read` / `bash_run`），加上少量 OpenClaw 风格别名。

use crate::compat::tool_aliases::lookup;
use crate::compat::{SkillCompat, SourceEcosystem};

const ALIASES: &[(&str, &str)] = &[
    // 与 hermes 一致的部分
    ("fs_read", "fs.read"),
    ("file_read", "fs.read"),
    ("fs_write", "fs.write"),
    ("file_write", "fs.write"),
    ("bash_run", "cmd.run"),
    ("shell_exec", "cmd.run"),
    ("browser_navigate", "browser.navigate"),
    // OpenClaw 风格 — 一些 skill 直接用 `cli_run`/`run_cmd` 包装系统命令
    ("cli_run", "cmd.run"),
    ("run_cmd", "cmd.run"),
];

#[derive(Debug, Default)]
pub struct OpenClawTranslator;

impl SkillCompat for OpenClawTranslator {
    fn ecosystem(&self) -> SourceEcosystem {
        SourceEcosystem::OpenClaw
    }

    fn translate_tool_name(&self, llm_name: &str) -> Option<String> {
        lookup(ALIASES, llm_name)
    }

    fn known_aliases(&self) -> &[&'static str] {
        &KNOWN_ALIASES
    }
}

static KNOWN_ALIASES: [&str; 9] = [
    "fs_read",
    "file_read",
    "fs_write",
    "file_write",
    "bash_run",
    "shell_exec",
    "browser_navigate",
    "cli_run",
    "run_cmd",
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
    fn openclaw_translator_maps_fs_read_and_cli_run() {
        let t = OpenClawTranslator;
        assert_eq!(t.translate_tool_name("fs_read").as_deref(), Some("fs.read"));
        assert_eq!(t.translate_tool_name("cli_run").as_deref(), Some("cmd.run"));
    }

    #[test]
    fn openclaw_translator_ecosystem() {
        let t = OpenClawTranslator;
        assert_eq!(t.ecosystem(), SourceEcosystem::OpenClaw);
    }
}
