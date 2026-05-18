//! 各 ecosystem 的 translator 实现。

pub mod anthropic;
pub mod claude_code;
pub mod hermes;
pub mod openclaw;
pub mod superpowers;

pub use anthropic::AnthropicTranslator;
pub use claude_code::ClaudeCodeTranslator;
pub use hermes::HermesTranslator;
pub use openclaw::OpenClawTranslator;
pub use superpowers::SuperpowersTranslator;
