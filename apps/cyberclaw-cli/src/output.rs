//! CLI 输出格式模块
//!
//! 提供统一的输出格式化支持，包括人类可读文本和 JSON 序列化。

use clap::ValueEnum;
use serde::Serialize;

/// CLI 输出格式
///
/// 控制命令输出的序列化方式。默认为 `text`，适合终端直接阅读；
/// `json` 适合脚本集成和机器消费。
///
/// # Examples
///
/// ```
/// use cyberclaw_cli::output::OutputFormat;
/// let fmt = OutputFormat::Text;
/// assert!(matches!(fmt, OutputFormat::Text));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum OutputFormat {
    /// 人类可读的文本格式（默认）
    #[default]
    Text,
    /// 机器友好的 JSON 格式
    Json,
}

/// 将任意可序列化值输出为 JSON 字符串（pretty-printed）。
///
/// # Errors
///
/// 序列化失败时返回 `serde_json::Error`。
#[allow(dead_code)]
pub fn print_json<T: Serialize>(value: &T) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(value)?;
    println!("{}", json);
    Ok(())
}

/// 格式化并输出错误信息（文本模式）。
#[allow(dead_code)]
pub fn print_error(msg: &str) {
    eprintln!("error: {}", msg);
}

/// 简单表格行输出辅助：打印左对齐的 `key: value` 对。
///
/// 用于 `OutputFormat::Text` 模式下的结构化信息展示。
pub fn print_field(label: &str, value: &str) {
    println!("{:<12} {}", format!("{}:", label), value);
}

/// 打印表格分隔线。
pub fn print_separator() {
    println!("{}", "-".repeat(40));
}
