//! Hermes Agent skill 的工具名翻译。
//!
//! Hermes 用 snake_case 工具名（`browser_navigate` / `fs_read` / `bash_run`），
//! cyberclaw facade 用点号命名（`browser.navigate` / `fs.read` / `cmd.run`）。

use crate::compat::tool_aliases::{aliases_only, lookup};
use crate::compat::{SkillCompat, SourceEcosystem};

/// hermes alias → cyberclaw facade 静态表。
///
/// 修改时同步更新 `known_aliases()`（通过 `aliases_only` 自动派生）。
const ALIASES: &[(&str, &str)] = &[
    // browser
    ("browser_navigate", "browser.navigate"),
    ("browser_snapshot", "browser.screenshot"),
    ("browser_click", "browser.click"),
    ("browser_type", "browser.fill"),
    ("browser_fill", "browser.fill"),
    ("browser_evaluate", "browser.evaluate"),
    ("browser_console", "browser.evaluate"),
    // fs
    ("fs_read", "fs.read"),
    ("file_read", "fs.read"),
    ("fs_write", "fs.write"),
    ("file_write", "fs.write"),
    ("fs_list", "search.glob"),
    ("fs_search", "search.grep"),
    // shell
    ("bash_run", "cmd.run"),
    ("shell_exec", "cmd.run"),
    ("python_exec", "cmd.run"),
];

#[derive(Debug, Default)]
pub struct HermesTranslator {
    known: once_cell::OnceCell<Vec<&'static str>>,
}

impl HermesTranslator {
    fn cached_aliases(&self) -> &[&'static str] {
        self.known.get_or_init(|| aliases_only(ALIASES))
    }
}

impl SkillCompat for HermesTranslator {
    fn ecosystem(&self) -> SourceEcosystem {
        SourceEcosystem::Hermes
    }

    fn translate_tool_name(&self, llm_name: &str) -> Option<String> {
        lookup(ALIASES, llm_name)
    }

    fn known_aliases(&self) -> &[&'static str] {
        self.cached_aliases()
    }
}

/// 暴露的 alias 数量（供启动报告 / 测试断言用）。
pub const ALIAS_COUNT: usize = ALIASES.len();

mod once_cell {
    /// 极简 OnceCell — 仅本模块内部使用。
    /// 不引入 `once_cell` crate（任务约束："不引入新 deps"）。
    use std::cell::UnsafeCell;
    use std::sync::Once;

    pub struct OnceCell<T> {
        once: Once,
        value: UnsafeCell<Option<T>>,
    }

    impl<T> Default for OnceCell<T> {
        fn default() -> Self {
            Self {
                once: Once::new(),
                value: UnsafeCell::new(None),
            }
        }
    }

    // SAFETY: `Once` 保证 `value` 只被写入一次，读取时只在写入完成后发生。
    unsafe impl<T: Send + Sync> Sync for OnceCell<T> {}
    unsafe impl<T: Send> Send for OnceCell<T> {}

    impl<T> OnceCell<T> {
        pub fn get_or_init<F: FnOnce() -> T>(&self, init: F) -> &T {
            self.once.call_once(|| {
                // SAFETY: Once 串行化此处写入。
                unsafe {
                    *self.value.get() = Some(init());
                }
            });
            // SAFETY: 写入已完成。
            unsafe {
                (*self.value.get())
                    .as_ref()
                    .expect("initialized by call_once")
            }
        }
    }

    impl<T: std::fmt::Debug> std::fmt::Debug for OnceCell<T> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("OnceCell").finish()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hermes_translator_maps_browser_navigate() {
        let t = HermesTranslator::default();
        assert_eq!(
            t.translate_tool_name("browser_navigate"),
            Some("browser.navigate".to_string())
        );
    }

    #[test]
    fn hermes_translator_maps_fs_read_and_write() {
        let t = HermesTranslator::default();
        assert_eq!(t.translate_tool_name("fs_read").as_deref(), Some("fs.read"));
        assert_eq!(
            t.translate_tool_name("file_write").as_deref(),
            Some("fs.write")
        );
    }

    #[test]
    fn hermes_translator_maps_bash_to_cmd_run() {
        let t = HermesTranslator::default();
        assert_eq!(
            t.translate_tool_name("bash_run").as_deref(),
            Some("cmd.run")
        );
        assert_eq!(
            t.translate_tool_name("shell_exec").as_deref(),
            Some("cmd.run")
        );
    }

    #[test]
    fn hermes_translator_returns_none_for_unknown() {
        let t = HermesTranslator::default();
        assert!(t.translate_tool_name("totally_unknown").is_none());
    }

    #[test]
    fn hermes_translator_ecosystem_is_hermes() {
        let t = HermesTranslator::default();
        assert_eq!(t.ecosystem(), SourceEcosystem::Hermes);
    }

    #[test]
    fn hermes_translator_known_aliases_non_empty() {
        let t = HermesTranslator::default();
        assert!(!t.known_aliases().is_empty());
        assert!(t.known_aliases().contains(&"browser_navigate"));
    }
}
