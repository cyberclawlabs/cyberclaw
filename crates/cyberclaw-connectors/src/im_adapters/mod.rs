//! # IM Platform Adapters
//!
//! 各 IM 平台的适配器实现。每个适配器负责处理对应平台特有的 API 差异。

pub mod discord;
pub mod lark;
pub mod line;
pub mod slack;
pub mod telegram;
pub mod wechat;

pub use discord::DiscordAdapter;
pub use lark::LarkAdapter;
pub use line::{LineAdapter, LineConfig};
pub use slack::SlackAdapter;
pub use telegram::{TelegramAdapter, TelegramConfig};
pub use wechat::WechatAdapter;
