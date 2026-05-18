//! SkillCompatLayer — generic skill-ecosystem compatibility translator.
//!
//! 多个外源 skill 生态（hermes / anthropic / claude_code / openclaw / superpowers）
//! 的 `SKILL.md` 引用各自风格的工具名（如 hermes 的 `browser_navigate`、anthropic
//! 的 `Bash`），但 cyberclaw 的 facade 命名为 `browser.navigate` / `cmd.run`。
//!
//! 本模块在运行时做工具名翻译，不修改 SKILL.md 原文（保留 attribution + 文档可读性）。
//!
//! ## 设计要点
//!
//! - [`SourceEcosystem`]：枚举 skill 来源生态
//! - [`SkillCompat`]：单个生态的翻译适配 trait
//! - [`SkillCompatRegistry`]：按 ecosystem 检索翻译器的容器
//! - [`source_detector`]：从 frontmatter / metadata 推断 source ecosystem
//! - [`translators`]：各生态具体实现
//!
//! ## 关键不变量
//!
//! 1. 翻译只发生在 LLM tool name → cyberclaw facade 名这一步
//! 2. 若没有匹配的 alias，返回 `None` 表示透传（不强制改名）
//! 3. `Native` 与 `Unknown` 不做翻译（透传）

use std::collections::HashMap;
use std::sync::Arc;

pub mod source_detector;
pub mod tool_aliases;
pub mod translators;

pub use source_detector::detect_source_ecosystem;

/// 标识一个 skill 来源生态。
///
/// 用于在加载 skill 时记录其原生工具命名风格，从而决定运行时该用哪一组 alias。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceEcosystem {
    /// Hermes Agent (`hermes-agent/skills/*`)
    Hermes,
    /// Anthropic 官方 claude-skill 风格（`Bash` / `Read` / `Write` 等大写工具名）
    Anthropic,
    /// Claude Code skill（已有 loader 接口）
    ClaudeCode,
    /// OpenClaw skill
    OpenClaw,
    /// Superpowers skill 包（多复用 anthropic 命名）
    Superpowers,
    /// cyberclaw 自家 21 个 skill（无需翻译）
    Native,
    /// 无法识别 — 退化为透传
    Unknown,
}

impl SourceEcosystem {
    /// 人类可读名称，用于日志与统计。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hermes => "hermes",
            Self::Anthropic => "anthropic",
            Self::ClaudeCode => "claude_code",
            Self::OpenClaw => "openclaw",
            Self::Superpowers => "superpowers",
            Self::Native => "native",
            Self::Unknown => "unknown",
        }
    }
}

/// 单个生态的翻译适配 trait。
///
/// 实现者负责把该生态的 LLM 工具名映射到 cyberclaw facade。
pub trait SkillCompat: Send + Sync {
    /// 这个 translator 服务于哪个 ecosystem。
    fn ecosystem(&self) -> SourceEcosystem;

    /// 把 LLM 用的工具名翻译为 cyberclaw facade 名。
    ///
    /// 若该生态从未声明该 alias，返回 `None` 表示不要翻译（透传）。
    fn translate_tool_name(&self, llm_name: &str) -> Option<String>;

    /// 已知 alias 列表（用于诊断、prompt 注入、测试枚举）。
    fn known_aliases(&self) -> &[&'static str];
}

/// 按 ecosystem 检索 translator 的注册表。
///
/// 默认包含全部内置 translator；可通过 [`SkillCompatRegistry::with_default`] 创建。
pub struct SkillCompatRegistry {
    translators: HashMap<SourceEcosystem, Arc<dyn SkillCompat>>,
}

impl SkillCompatRegistry {
    /// 空注册表。
    pub fn new() -> Self {
        Self {
            translators: HashMap::new(),
        }
    }

    /// 预填充内置 translator。
    pub fn with_default() -> Self {
        let mut reg = Self::new();
        reg.register(Arc::new(translators::HermesTranslator::default()));
        reg.register(Arc::new(translators::AnthropicTranslator));
        reg.register(Arc::new(translators::ClaudeCodeTranslator));
        reg.register(Arc::new(translators::OpenClawTranslator));
        reg.register(Arc::new(translators::SuperpowersTranslator));
        reg
    }

    /// 注册一个 translator（按其 `ecosystem()` 自动归类）。后注册者覆盖。
    pub fn register(&mut self, translator: Arc<dyn SkillCompat>) {
        self.translators.insert(translator.ecosystem(), translator);
    }

    /// 取得对应 ecosystem 的 translator（若已注册）。
    pub fn get(&self, ecosystem: SourceEcosystem) -> Option<Arc<dyn SkillCompat>> {
        self.translators.get(&ecosystem).cloned()
    }

    /// 翻译 `llm_name`。规则：
    ///
    /// 1. 优先查每个绑定的 ecosystem translator
    /// 2. 任一返回 `Some(facade)` 即作为最终结果
    /// 3. 全部 `None` 则返回 `None` 表示透传（保兼容）
    pub fn translate_for(&self, bound: &[SourceEcosystem], llm_name: &str) -> Option<String> {
        for eco in bound {
            if let Some(t) = self.translators.get(eco) {
                if let Some(facade) = t.translate_tool_name(llm_name) {
                    return Some(facade);
                }
            }
        }
        None
    }
}

impl Default for SkillCompatRegistry {
    fn default() -> Self {
        Self::with_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_default_contains_five_translators() {
        let reg = SkillCompatRegistry::with_default();
        assert!(reg.get(SourceEcosystem::Hermes).is_some());
        assert!(reg.get(SourceEcosystem::Anthropic).is_some());
        assert!(reg.get(SourceEcosystem::ClaudeCode).is_some());
        assert!(reg.get(SourceEcosystem::OpenClaw).is_some());
        assert!(reg.get(SourceEcosystem::Superpowers).is_some());
        // Native / Unknown 应透传，没有 translator
        assert!(reg.get(SourceEcosystem::Native).is_none());
        assert!(reg.get(SourceEcosystem::Unknown).is_none());
    }

    #[test]
    fn translate_for_falls_through_when_no_match() {
        let reg = SkillCompatRegistry::with_default();
        let out = reg.translate_for(&[SourceEcosystem::Hermes], "totally_unknown_tool_xyz");
        assert!(out.is_none());
    }

    #[test]
    fn translate_for_first_match_wins() {
        let reg = SkillCompatRegistry::with_default();
        // hermes 先于 anthropic 列出 — `fs_read` 是 hermes alias
        let out = reg
            .translate_for(
                &[SourceEcosystem::Hermes, SourceEcosystem::Anthropic],
                "fs_read",
            )
            .expect("hermes alias should resolve");
        assert_eq!(out, "fs.read");
    }

    #[test]
    fn ecosystem_serde_roundtrip() {
        let eco = SourceEcosystem::Hermes;
        let s = serde_json::to_string(&eco).unwrap();
        assert_eq!(s, "\"hermes\"");
        let back: SourceEcosystem = serde_json::from_str(&s).unwrap();
        assert_eq!(back, eco);
    }
}
