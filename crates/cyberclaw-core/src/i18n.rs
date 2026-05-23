//! 国际化支持 (i18n) — 编译时字符串表，支持 en / zh 两种语言。
//!
//! # 设计原则
//!
//! - **无运行时 IO**：字符串表在编译期写死，不解析 YAML/TOML 文件，无 IO 失败风险。
//! - **无新重型依赖**：仅使用 `once_cell`（workspace 已有）和标准库。
//! - **向后兼容**：默认语言为 En，英语用户无感知变化。
//! - **最小覆盖范围**：仅覆盖 20 个高频用户可见字符串（v1）。
//!
//! # 使用示例
//!
//! ```rust
//! use cyberclaw_core::i18n::{t, set_locale, Locale};
//!
//! // 查询当前 locale 字符串
//! println!("{}", t("system.ready"));
//!
//! // 切换语言
//! set_locale(Locale::Zh);
//! println!("{}", t("approval.approved")); // 输出：已批准。
//!
//! // 带 {} 占位符的字符串
//! let msg = t("error.unknown_command").replace("{}", "/foo");
//! ```
//!
//! # Locale 解析顺序
//!
//! 1. `CYBERCLAW_LOCALE` 环境变量
//! 2. `LANG` 环境变量
//! 3. 默认 `en`

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::RwLock;

// ---------------------------------------------------------------------------
// Locale 枚举
// ---------------------------------------------------------------------------

/// 支持的语言种类（v1：en + zh）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Locale {
    /// English (default)
    En,
    /// Simplified Chinese (简体中文)
    Zh,
}

impl Locale {
    /// 从环境变量推断 locale。
    ///
    /// 解析顺序：`CYBERCLAW_LOCALE` → `LANG` → `En`。
    pub fn from_env() -> Self {
        let val = std::env::var("CYBERCLAW_LOCALE")
            .or_else(|_| std::env::var("LANG"))
            .unwrap_or_default();
        Self::parse_tag(&val)
    }

    /// 从字符串解析 locale（宽松匹配）。
    pub fn parse_tag(s: &str) -> Self {
        let s = s.trim().to_ascii_lowercase();
        if s.starts_with("zh")
            || s == "chinese"
            || s == "mandarin"
            || s == "zh-cn"
            || s == "zh_cn"
            || s == "zh-hans"
            || s == "zh-tw"
            || s == "zh_cn.utf-8"
            || s == "zh_cn.utf8"
        {
            Self::Zh
        } else {
            Self::En
        }
    }
}

// ---------------------------------------------------------------------------
// 全局 locale 状态
// ---------------------------------------------------------------------------

static LOCALE: Lazy<RwLock<Locale>> = Lazy::new(|| RwLock::new(Locale::from_env()));

/// 设置全局 locale（线程安全）。
pub fn set_locale(l: Locale) {
    *LOCALE.write().unwrap_or_else(|e| e.into_inner()) = l;
}

/// 获取当前全局 locale（线程安全）。
pub fn current_locale() -> Locale {
    *LOCALE.read().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// 字符串查询
// ---------------------------------------------------------------------------

/// 查询当前 locale 的用户可见字符串。
///
/// - 若 zh 中找不到 key，自动回退到 en。
/// - 若 en 也找不到，返回空字符串 `""`（确保不崩溃，调用方可用 key 作为占位）。
/// - `{}` 占位符由调用方用 `.replace("{}", &arg)` 或 `format!` 展开。
/// - key 必须为 `'static`（字面量），以确保回退时返回 `&'static str` 有效。
pub fn t(key: &'static str) -> &'static str {
    let locale = current_locale();
    STRINGS.get(&(locale, key)).copied().unwrap_or_else(|| {
        // zh key 缺失时回退到 en
        STRINGS.get(&(Locale::En, key)).copied().unwrap_or(key)
    })
}

// ---------------------------------------------------------------------------
// 编译期字符串表（20 个 key × 2 个语言）
// ---------------------------------------------------------------------------

static STRINGS: Lazy<HashMap<(Locale, &'static str), &'static str>> = Lazy::new(|| {
    let mut m: HashMap<(Locale, &'static str), &'static str> = HashMap::new();

    macro_rules! add {
        ($key:expr, $en:expr, $zh:expr) => {
            m.insert((Locale::En, $key), $en);
            m.insert((Locale::Zh, $key), $zh);
        };
    }

    // --- approval ---
    add!(
        "approval.prompt",
        "Approve this action? [y/N]",
        "是否批准此操作？[y/N]"
    );
    add!("approval.approved", "Approved.", "已批准。");
    add!("approval.rejected", "Rejected.", "已拒绝。");
    add!("approval.timeout", "Approval timed out.", "审批超时。");

    // --- slash_commands ---
    add!("slash.help", "Available commands:", "可用命令：");
    add!("slash.usage", "Session usage:", "会话用量：");
    add!(
        "slash.undo_success",
        "Last turn undone.",
        "已撤销最近一轮。"
    );
    add!("slash.queue_empty", "No queued messages.", "队列为空。");
    add!(
        "slash.details_off",
        "Details panel hidden.",
        "详情面板已隐藏。"
    );
    add!(
        "slash.details_on",
        "Details panel shown.",
        "详情面板已显示。"
    );

    // --- errors ---
    add!(
        "error.connection_failed",
        "Connection failed: {}",
        "连接失败：{}"
    );
    add!("error.invalid_input", "Invalid input.", "无效输入。");
    add!(
        "error.no_active_session",
        "No active session.",
        "没有活动会话。"
    );
    add!(
        "error.unknown_command",
        "Unknown command: {}",
        "未知命令：{}"
    );

    // --- system ---
    add!("system.ready", "Ready.", "准备就绪。");
    add!(
        "system.connecting",
        "Connecting to server...",
        "正在连接服务器..."
    );
    add!("system.shutting_down", "Shutting down.", "正在关闭。");

    // --- agent ---
    add!("agent.thinking", "Thinking...", "思考中...");
    add!("agent.tool_call", "Calling tool: {}", "调用工具：{}");
    add!(
        "agent.waiting_approval",
        "Waiting for approval...",
        "等待审批..."
    );

    m
});

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_locale_is_en() {
        // 不设置环境变量时，from_str("") 应返回 En
        assert_eq!(Locale::parse_tag(""), Locale::En);
        assert_eq!(Locale::parse_tag("en_US.UTF-8"), Locale::En);
    }

    #[test]
    fn test_zh_lookup_returns_chinese() {
        assert_eq!(
            STRINGS.get(&(Locale::Zh, "approval.approved")).copied(),
            Some("已批准。")
        );
        assert_eq!(
            STRINGS.get(&(Locale::Zh, "system.ready")).copied(),
            Some("准备就绪。")
        );
    }

    #[test]
    fn test_unknown_key_returns_key_itself() {
        // 使用显式 En lookup 验证不存在的 key 返回 key 自身
        let key = "nonexistent.key.xyz";
        let result = STRINGS.get(&(Locale::En, key)).copied().unwrap_or(key);
        assert_eq!(result, key);
    }

    #[test]
    fn test_zh_missing_key_falls_back_to_en() {
        // 所有 20 个 key 在 zh 中都存在，此处用一个不在表中的 zh key 验证回退
        // 通过直接调用 STRINGS.get 缺失路径来模拟
        let key = "does.not.exist.in.zh";
        let zh_val = STRINGS.get(&(Locale::Zh, key)).copied();
        let en_val = STRINGS.get(&(Locale::En, key)).copied();
        // 两者均不存在，t() 最终回退到 key 本身
        assert!(zh_val.is_none());
        assert!(en_val.is_none());

        // 验证 t() 的回退行为（全局 locale 临时切到 Zh）
        set_locale(Locale::Zh);
        let result = t(key);
        assert_eq!(result, key);
        // 恢复
        set_locale(Locale::En);
    }

    #[test]
    fn test_from_env_zh_cn_returns_zh() {
        assert_eq!(Locale::parse_tag("zh_CN.UTF-8"), Locale::Zh);
        assert_eq!(Locale::parse_tag("zh"), Locale::Zh);
        assert_eq!(Locale::parse_tag("zh_CN"), Locale::Zh);
        assert_eq!(Locale::parse_tag("zh-Hans"), Locale::Zh);
        assert_eq!(Locale::parse_tag("ZH_CN"), Locale::Zh);
    }
}
