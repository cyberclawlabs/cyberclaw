//! LLM 费用估算模块
//!
//! 基于各主流 LLM 提供商的公开定价（2026 Q1），将 token 使用量转换为 USD 费用估算。
//!
//! # 支持的提供商
//!
//! - Anthropic (Claude 3 / 3.5 / 4 系列)
//! - OpenAI (GPT-4o / o3 系列)
//! - DeepSeek
//! - MiniMax
//! - Volcengine Ark (deepseek-v3 / deepseek-r1)
//! - Google Gemini
//! - AWS Bedrock
//!
//! # 前缀匹配
//!
//! `lookup_pricing` 先做精确匹配，再做前缀匹配（最长前缀优先）。
//! 未知模型返回 `None`，`estimate_cost` 对应返回 `pricing_known = false`，费用为 0。

use once_cell::sync::Lazy;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// 核心类型
// ---------------------------------------------------------------------------

/// 单个模型的定价条目（单位：USD / 百万 tokens）。
#[derive(Debug, Clone, Copy)]
pub struct PricingEntry {
    /// Input tokens 每百万 USD
    pub input_per_million_usd: f64,
    /// Output tokens 每百万 USD
    pub output_per_million_usd: f64,
    /// Cache read tokens 每百万 USD（可选，Anthropic/OpenAI 风格）
    pub cache_read_per_million_usd: Option<f64>,
    /// Cache write tokens 每百万 USD（可选，Anthropic 风格）
    pub cache_write_per_million_usd: Option<f64>,
}

/// 一次 LLM 调用的 token 使用量（规范化格式）。
#[derive(Debug, Clone, Default)]
pub struct CanonicalUsage {
    /// 正常 input tokens（不含 cache）
    pub input_tokens: u64,
    /// Output tokens
    pub output_tokens: u64,
    /// 从 cache 读取的 input tokens
    pub cache_read_tokens: u64,
    /// 写入 cache 的 input tokens
    pub cache_write_tokens: u64,
}

/// 单次调用的费用分解（USD）。
#[derive(Debug, Clone, Default)]
pub struct CostResult {
    /// Input tokens 费用（USD）
    pub input_cost_usd: f64,
    /// Output tokens 费用（USD）
    pub output_cost_usd: f64,
    /// Cache read 费用（USD）
    pub cache_read_cost_usd: f64,
    /// Cache write 费用（USD）
    pub cache_write_cost_usd: f64,
    /// 合计（USD）
    pub total_cost_usd: f64,
    /// 所用模型名称
    pub model: String,
    /// 是否在定价表中找到该模型；false 时所有费用均为 0
    pub pricing_known: bool,
}

// ---------------------------------------------------------------------------
// 定价表（静态，延迟初始化）
// ---------------------------------------------------------------------------

/// 精确模型名 -> 定价条目。
///
/// 来源：hermes usage_pricing.py `_OFFICIAL_DOCS_PRICING`，2026-Q1 公开定价快照。
static PRICING_TABLE: Lazy<HashMap<&'static str, PricingEntry>> = Lazy::new(|| {
    let mut m = HashMap::new();

    // ── Anthropic Claude 4.7 ─────────────────────────────────────────────
    // $5/$25 input/output (新 tokenizer；Opus 4.5/4.6/4.7 共享价格)
    // Source: https://platform.claude.com/docs/en/about-claude/pricing
    let claude_opus_4x = PricingEntry {
        input_per_million_usd: 5.00,
        output_per_million_usd: 25.00,
        cache_read_per_million_usd: Some(0.50),
        cache_write_per_million_usd: Some(6.25),
    };
    m.insert("claude-opus-4-7", claude_opus_4x);
    m.insert("claude-opus-4-7-20250507", claude_opus_4x);

    // ── Anthropic Claude 4.6 ─────────────────────────────────────────────
    m.insert("claude-opus-4-6", claude_opus_4x);
    m.insert("claude-opus-4-6-20250414", claude_opus_4x);

    let claude_sonnet_4x = PricingEntry {
        input_per_million_usd: 3.00,
        output_per_million_usd: 15.00,
        cache_read_per_million_usd: Some(0.30),
        cache_write_per_million_usd: Some(3.75),
    };
    m.insert("claude-sonnet-4-6", claude_sonnet_4x);
    m.insert("claude-sonnet-4-6-20250414", claude_sonnet_4x);

    // ── Anthropic Claude 4.5 ─────────────────────────────────────────────
    m.insert("claude-opus-4-5", claude_opus_4x);
    m.insert("claude-sonnet-4-5", claude_sonnet_4x);

    let claude_haiku_4x = PricingEntry {
        input_per_million_usd: 1.00,
        output_per_million_usd: 5.00,
        cache_read_per_million_usd: Some(0.10),
        cache_write_per_million_usd: Some(1.25),
    };
    m.insert("claude-haiku-4-5", claude_haiku_4x);

    // ── Anthropic Claude 4 / 4.1 ─────────────────────────────────────────
    // claude-opus-4 正式版：$15/$75 (同 claude-3-opus)
    let claude_opus_4_orig = PricingEntry {
        input_per_million_usd: 15.00,
        output_per_million_usd: 75.00,
        cache_read_per_million_usd: Some(1.50),
        cache_write_per_million_usd: Some(18.75),
    };
    m.insert("claude-opus-4-20250514", claude_opus_4_orig);
    m.insert("claude-sonnet-4-20250514", claude_sonnet_4x);

    // ── Anthropic Claude 3.5 ─────────────────────────────────────────────
    let claude_35_sonnet = PricingEntry {
        input_per_million_usd: 3.00,
        output_per_million_usd: 15.00,
        cache_read_per_million_usd: Some(0.30),
        cache_write_per_million_usd: Some(3.75),
    };
    m.insert("claude-3-5-sonnet-20241022", claude_35_sonnet);
    m.insert("claude-3-5-sonnet-20240620", claude_35_sonnet);

    let claude_35_haiku = PricingEntry {
        input_per_million_usd: 0.80,
        output_per_million_usd: 4.00,
        cache_read_per_million_usd: Some(0.08),
        cache_write_per_million_usd: Some(1.00),
    };
    m.insert("claude-3-5-haiku-20241022", claude_35_haiku);

    // ── Anthropic Claude 3 ───────────────────────────────────────────────
    let claude_3_opus = PricingEntry {
        input_per_million_usd: 15.00,
        output_per_million_usd: 75.00,
        cache_read_per_million_usd: Some(1.50),
        cache_write_per_million_usd: Some(18.75),
    };
    m.insert("claude-3-opus-20240229", claude_3_opus);

    let claude_3_sonnet = PricingEntry {
        input_per_million_usd: 3.00,
        output_per_million_usd: 15.00,
        cache_read_per_million_usd: Some(0.30),
        cache_write_per_million_usd: Some(3.75),
    };
    m.insert("claude-3-sonnet-20240229", claude_3_sonnet);

    let claude_3_haiku = PricingEntry {
        input_per_million_usd: 0.25,
        output_per_million_usd: 1.25,
        cache_read_per_million_usd: Some(0.03),
        cache_write_per_million_usd: Some(0.30),
    };
    m.insert("claude-3-haiku-20240307", claude_3_haiku);

    // ── OpenAI ───────────────────────────────────────────────────────────
    // Source: https://openai.com/api/pricing/ (2026-03-16 snapshot)
    m.insert(
        "gpt-4o",
        PricingEntry {
            input_per_million_usd: 2.50,
            output_per_million_usd: 10.00,
            cache_read_per_million_usd: Some(1.25),
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "gpt-4o-mini",
        PricingEntry {
            input_per_million_usd: 0.15,
            output_per_million_usd: 0.60,
            cache_read_per_million_usd: Some(0.075),
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "gpt-4.1",
        PricingEntry {
            input_per_million_usd: 2.00,
            output_per_million_usd: 8.00,
            cache_read_per_million_usd: Some(0.50),
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "gpt-4.1-mini",
        PricingEntry {
            input_per_million_usd: 0.40,
            output_per_million_usd: 1.60,
            cache_read_per_million_usd: Some(0.10),
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "gpt-4.1-nano",
        PricingEntry {
            input_per_million_usd: 0.10,
            output_per_million_usd: 0.40,
            cache_read_per_million_usd: Some(0.025),
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "gpt-4-turbo",
        PricingEntry {
            input_per_million_usd: 10.00,
            output_per_million_usd: 30.00,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "gpt-3.5-turbo",
        PricingEntry {
            input_per_million_usd: 0.50,
            output_per_million_usd: 1.50,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "o1-preview",
        PricingEntry {
            input_per_million_usd: 15.00,
            output_per_million_usd: 60.00,
            cache_read_per_million_usd: Some(7.50),
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "o1-mini",
        PricingEntry {
            input_per_million_usd: 1.10,
            output_per_million_usd: 4.40,
            cache_read_per_million_usd: Some(0.55),
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "o3",
        PricingEntry {
            input_per_million_usd: 10.00,
            output_per_million_usd: 40.00,
            cache_read_per_million_usd: Some(2.50),
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "o3-mini",
        PricingEntry {
            input_per_million_usd: 1.10,
            output_per_million_usd: 4.40,
            cache_read_per_million_usd: Some(0.55),
            cache_write_per_million_usd: None,
        },
    );

    // ── DeepSeek ─────────────────────────────────────────────────────────
    // Source: https://api-docs.deepseek.com/quick_start/pricing (2026-03-16)
    m.insert(
        "deepseek-chat",
        PricingEntry {
            input_per_million_usd: 0.14,
            output_per_million_usd: 0.28,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "deepseek-reasoner",
        PricingEntry {
            input_per_million_usd: 0.55,
            output_per_million_usd: 2.19,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );

    // ── MiniMax ──────────────────────────────────────────────────────────
    m.insert(
        "abab-6.5-chat",
        PricingEntry {
            input_per_million_usd: 0.30,
            output_per_million_usd: 1.20,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "abab-6.5s-chat",
        PricingEntry {
            input_per_million_usd: 0.10,
            output_per_million_usd: 0.10,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "minimax-m2.7",
        PricingEntry {
            input_per_million_usd: 0.30,
            output_per_million_usd: 1.20,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );

    // ── Volcengine Ark (DeepSeek via Ark) ────────────────────────────────
    // 定价同 DeepSeek 官方 API；Ark 通道转发，按 DeepSeek 计费
    m.insert(
        "ark-deepseek-v3",
        PricingEntry {
            input_per_million_usd: 0.14,
            output_per_million_usd: 0.28,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "ark-deepseek-r1",
        PricingEntry {
            input_per_million_usd: 0.55,
            output_per_million_usd: 2.19,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );

    // ── Google Gemini ────────────────────────────────────────────────────
    // Source: https://ai.google.dev/pricing (2026-03-16)
    m.insert(
        "gemini-2.5-pro",
        PricingEntry {
            input_per_million_usd: 1.25,
            output_per_million_usd: 10.00,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "gemini-2.5-flash",
        PricingEntry {
            input_per_million_usd: 0.15,
            output_per_million_usd: 0.60,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "gemini-2.0-flash",
        PricingEntry {
            input_per_million_usd: 0.10,
            output_per_million_usd: 0.40,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "gemini-1.5-pro",
        PricingEntry {
            input_per_million_usd: 1.25,
            output_per_million_usd: 5.00,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "gemini-1.5-flash",
        PricingEntry {
            input_per_million_usd: 0.075,
            output_per_million_usd: 0.30,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );

    // ── AWS Bedrock ──────────────────────────────────────────────────────
    // Source: https://aws.amazon.com/bedrock/pricing/ (2026-04 snapshot)
    m.insert(
        "anthropic.claude-opus-4-6",
        PricingEntry {
            input_per_million_usd: 15.00,
            output_per_million_usd: 75.00,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "anthropic.claude-sonnet-4-6",
        PricingEntry {
            input_per_million_usd: 3.00,
            output_per_million_usd: 15.00,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "anthropic.claude-sonnet-4-5",
        PricingEntry {
            input_per_million_usd: 3.00,
            output_per_million_usd: 15.00,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "anthropic.claude-haiku-4-5",
        PricingEntry {
            input_per_million_usd: 0.80,
            output_per_million_usd: 4.00,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "amazon.nova-pro",
        PricingEntry {
            input_per_million_usd: 0.80,
            output_per_million_usd: 3.20,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "amazon.nova-lite",
        PricingEntry {
            input_per_million_usd: 0.06,
            output_per_million_usd: 0.24,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );
    m.insert(
        "amazon.nova-micro",
        PricingEntry {
            input_per_million_usd: 0.035,
            output_per_million_usd: 0.14,
            cache_read_per_million_usd: None,
            cache_write_per_million_usd: None,
        },
    );

    m
});

/// 前缀匹配表：前缀字符串 -> 定价条目（最长前缀优先匹配）。
///
/// 用于 `claude-3-5-sonnet-20241022` 这类带日期后缀的模型名，以便
/// 使用 `claude-3-5-sonnet-*` 这样的模糊搜索找到定价。
static PREFIX_TABLE: Lazy<Vec<(&'static str, PricingEntry)>> = Lazy::new(|| {
    let mut v: Vec<(&'static str, PricingEntry)> = vec![
        // Anthropic prefix groups
        (
            "claude-opus-4",
            PricingEntry {
                input_per_million_usd: 5.00,
                output_per_million_usd: 25.00,
                cache_read_per_million_usd: Some(0.50),
                cache_write_per_million_usd: Some(6.25),
            },
        ),
        (
            "claude-sonnet-4",
            PricingEntry {
                input_per_million_usd: 3.00,
                output_per_million_usd: 15.00,
                cache_read_per_million_usd: Some(0.30),
                cache_write_per_million_usd: Some(3.75),
            },
        ),
        (
            "claude-haiku-4",
            PricingEntry {
                input_per_million_usd: 1.00,
                output_per_million_usd: 5.00,
                cache_read_per_million_usd: Some(0.10),
                cache_write_per_million_usd: Some(1.25),
            },
        ),
        (
            "claude-3-5-sonnet",
            PricingEntry {
                input_per_million_usd: 3.00,
                output_per_million_usd: 15.00,
                cache_read_per_million_usd: Some(0.30),
                cache_write_per_million_usd: Some(3.75),
            },
        ),
        (
            "claude-3-5-haiku",
            PricingEntry {
                input_per_million_usd: 0.80,
                output_per_million_usd: 4.00,
                cache_read_per_million_usd: Some(0.08),
                cache_write_per_million_usd: Some(1.00),
            },
        ),
        (
            "claude-3-opus",
            PricingEntry {
                input_per_million_usd: 15.00,
                output_per_million_usd: 75.00,
                cache_read_per_million_usd: Some(1.50),
                cache_write_per_million_usd: Some(18.75),
            },
        ),
        (
            "claude-3-sonnet",
            PricingEntry {
                input_per_million_usd: 3.00,
                output_per_million_usd: 15.00,
                cache_read_per_million_usd: Some(0.30),
                cache_write_per_million_usd: Some(3.75),
            },
        ),
        (
            "claude-3-haiku",
            PricingEntry {
                input_per_million_usd: 0.25,
                output_per_million_usd: 1.25,
                cache_read_per_million_usd: Some(0.03),
                cache_write_per_million_usd: Some(0.30),
            },
        ),
        // OpenAI prefix groups
        (
            "gpt-4o-mini",
            PricingEntry {
                input_per_million_usd: 0.15,
                output_per_million_usd: 0.60,
                cache_read_per_million_usd: Some(0.075),
                cache_write_per_million_usd: None,
            },
        ),
        (
            "gpt-4o",
            PricingEntry {
                input_per_million_usd: 2.50,
                output_per_million_usd: 10.00,
                cache_read_per_million_usd: Some(1.25),
                cache_write_per_million_usd: None,
            },
        ),
        (
            "gpt-4.1-mini",
            PricingEntry {
                input_per_million_usd: 0.40,
                output_per_million_usd: 1.60,
                cache_read_per_million_usd: Some(0.10),
                cache_write_per_million_usd: None,
            },
        ),
        (
            "gpt-4.1-nano",
            PricingEntry {
                input_per_million_usd: 0.10,
                output_per_million_usd: 0.40,
                cache_read_per_million_usd: Some(0.025),
                cache_write_per_million_usd: None,
            },
        ),
        (
            "gpt-4.1",
            PricingEntry {
                input_per_million_usd: 2.00,
                output_per_million_usd: 8.00,
                cache_read_per_million_usd: Some(0.50),
                cache_write_per_million_usd: None,
            },
        ),
        (
            "gpt-4-turbo",
            PricingEntry {
                input_per_million_usd: 10.00,
                output_per_million_usd: 30.00,
                cache_read_per_million_usd: None,
                cache_write_per_million_usd: None,
            },
        ),
        (
            "gpt-4",
            PricingEntry {
                input_per_million_usd: 30.00,
                output_per_million_usd: 60.00,
                cache_read_per_million_usd: None,
                cache_write_per_million_usd: None,
            },
        ),
        (
            "gpt-3.5-turbo",
            PricingEntry {
                input_per_million_usd: 0.50,
                output_per_million_usd: 1.50,
                cache_read_per_million_usd: None,
                cache_write_per_million_usd: None,
            },
        ),
        (
            "o3-mini",
            PricingEntry {
                input_per_million_usd: 1.10,
                output_per_million_usd: 4.40,
                cache_read_per_million_usd: Some(0.55),
                cache_write_per_million_usd: None,
            },
        ),
        (
            "o3",
            PricingEntry {
                input_per_million_usd: 10.00,
                output_per_million_usd: 40.00,
                cache_read_per_million_usd: Some(2.50),
                cache_write_per_million_usd: None,
            },
        ),
        (
            "o1-mini",
            PricingEntry {
                input_per_million_usd: 1.10,
                output_per_million_usd: 4.40,
                cache_read_per_million_usd: Some(0.55),
                cache_write_per_million_usd: None,
            },
        ),
        (
            "o1-preview",
            PricingEntry {
                input_per_million_usd: 15.00,
                output_per_million_usd: 60.00,
                cache_read_per_million_usd: Some(7.50),
                cache_write_per_million_usd: None,
            },
        ),
        (
            "o1",
            PricingEntry {
                input_per_million_usd: 15.00,
                output_per_million_usd: 60.00,
                cache_read_per_million_usd: Some(7.50),
                cache_write_per_million_usd: None,
            },
        ),
        // Gemini prefix groups
        (
            "gemini-2.5-pro",
            PricingEntry {
                input_per_million_usd: 1.25,
                output_per_million_usd: 10.00,
                cache_read_per_million_usd: None,
                cache_write_per_million_usd: None,
            },
        ),
        (
            "gemini-2.5-flash",
            PricingEntry {
                input_per_million_usd: 0.15,
                output_per_million_usd: 0.60,
                cache_read_per_million_usd: None,
                cache_write_per_million_usd: None,
            },
        ),
        (
            "gemini-2.0-flash",
            PricingEntry {
                input_per_million_usd: 0.10,
                output_per_million_usd: 0.40,
                cache_read_per_million_usd: None,
                cache_write_per_million_usd: None,
            },
        ),
        (
            "gemini-1.5-pro",
            PricingEntry {
                input_per_million_usd: 1.25,
                output_per_million_usd: 5.00,
                cache_read_per_million_usd: None,
                cache_write_per_million_usd: None,
            },
        ),
        (
            "gemini-1.5-flash",
            PricingEntry {
                input_per_million_usd: 0.075,
                output_per_million_usd: 0.30,
                cache_read_per_million_usd: None,
                cache_write_per_million_usd: None,
            },
        ),
        // DeepSeek prefix groups
        (
            "deepseek-chat",
            PricingEntry {
                input_per_million_usd: 0.14,
                output_per_million_usd: 0.28,
                cache_read_per_million_usd: None,
                cache_write_per_million_usd: None,
            },
        ),
        (
            "deepseek-reasoner",
            PricingEntry {
                input_per_million_usd: 0.55,
                output_per_million_usd: 2.19,
                cache_read_per_million_usd: None,
                cache_write_per_million_usd: None,
            },
        ),
        // MiniMax prefix groups
        // MiniMax-M2.7-HighSpeed lowercases to "minimax-m2.7-highspeed" which
        // doesn't match the exact "minimax-m2.7" entry — catch all m2.* variants.
        (
            "minimax-m2",
            PricingEntry {
                input_per_million_usd: 0.30,
                output_per_million_usd: 1.20,
                cache_read_per_million_usd: None,
                cache_write_per_million_usd: None,
            },
        ),
        // abab-* prefix covers abab-6.5-chat, abab-6.5s-chat and future variants.
        // Use abab-6.5-chat pricing ($0.30/$1.20) as the conservative fallback.
        (
            "abab-",
            PricingEntry {
                input_per_million_usd: 0.30,
                output_per_million_usd: 1.20,
                cache_read_per_million_usd: None,
                cache_write_per_million_usd: None,
            },
        ),
        // Ark prefix groups
        (
            "ark-deepseek-v3",
            PricingEntry {
                input_per_million_usd: 0.14,
                output_per_million_usd: 0.28,
                cache_read_per_million_usd: None,
                cache_write_per_million_usd: None,
            },
        ),
        (
            "ark-deepseek-r1",
            PricingEntry {
                input_per_million_usd: 0.55,
                output_per_million_usd: 2.19,
                cache_read_per_million_usd: None,
                cache_write_per_million_usd: None,
            },
        ),
    ];
    // 降序排列：最长前缀优先
    v.sort_by_key(|item| std::cmp::Reverse(item.0.len()));
    v
});

// ---------------------------------------------------------------------------
// 公共 API
// ---------------------------------------------------------------------------

/// 查找模型定价。先精确匹配，再最长前缀匹配。
///
/// # 返回
///
/// - `Some(&PricingEntry)` — 找到定价
/// - `None` — 未知模型
pub fn lookup_pricing(model: &str) -> Option<&'static PricingEntry> {
    let lower = model.to_lowercase();
    let model_str = lower.as_str();

    // 1. 精确匹配
    if let Some(entry) = PRICING_TABLE.get(model_str) {
        return Some(entry);
    }

    // 2. 最长前缀匹配（PREFIX_TABLE 已按长度降序排列）
    for (prefix, entry) in PREFIX_TABLE.iter() {
        if model_str.starts_with(prefix) {
            return Some(entry);
        }
    }

    None
}

/// 估算一次 LLM 调用的费用。
///
/// 未知模型返回 `CostResult { pricing_known: false, total_cost_usd: 0.0, … }`。
pub fn estimate_cost(usage: &CanonicalUsage, model: &str) -> CostResult {
    match lookup_pricing(model) {
        None => CostResult {
            model: model.to_string(),
            pricing_known: false,
            ..Default::default()
        },
        Some(entry) => {
            let input_cost_usd =
                (usage.input_tokens as f64) * entry.input_per_million_usd / 1_000_000.0;
            let output_cost_usd =
                (usage.output_tokens as f64) * entry.output_per_million_usd / 1_000_000.0;
            let cache_read_cost_usd = entry
                .cache_read_per_million_usd
                .map(|rate| (usage.cache_read_tokens as f64) * rate / 1_000_000.0)
                .unwrap_or(0.0);
            let cache_write_cost_usd = entry
                .cache_write_per_million_usd
                .map(|rate| (usage.cache_write_tokens as f64) * rate / 1_000_000.0)
                .unwrap_or(0.0);
            let total_cost_usd =
                input_cost_usd + output_cost_usd + cache_read_cost_usd + cache_write_cost_usd;
            CostResult {
                input_cost_usd,
                output_cost_usd,
                cache_read_cost_usd,
                cache_write_cost_usd,
                total_cost_usd,
                model: model.to_string(),
                pricing_known: true,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// CostAccumulator
// ---------------------------------------------------------------------------

/// 会话级费用累计器。
///
/// 通过 `add_response` 逐次叠加每次 LLM 调用的费用，支持按模型分解。
/// 适合挂载到 agentic loop 状态或 TUI 应用状态中。
#[derive(Debug, Clone, Default)]
pub struct CostAccumulator {
    /// 累计 input tokens
    pub total_input_tokens: u64,
    /// 累计 output tokens
    pub total_output_tokens: u64,
    /// 累计 cache read tokens
    pub total_cache_read_tokens: u64,
    /// 累计 cache write tokens
    pub total_cache_write_tokens: u64,
    /// 累计 input USD 费用
    pub total_input_cost_usd: f64,
    /// 累计 output USD 费用
    pub total_output_cost_usd: f64,
    /// 累计 cache read USD 费用
    pub total_cache_read_cost_usd: f64,
    /// 累计 cache write USD 费用
    pub total_cache_write_cost_usd: f64,
    /// 累计总 USD 费用
    pub total_cost_usd: f64,
    /// 按模型分解的费用
    pub per_model_costs: HashMap<String, ModelCost>,
    /// 定价未知的模型列表（model_name -> total_tokens）
    pub unknown_models: HashMap<String, u64>,
}

/// 单模型费用摘要（用于 per_model_costs 的值）。
#[derive(Debug, Clone, Default)]
pub struct ModelCost {
    /// 该模型累计 USD 费用
    pub cost_usd: f64,
    /// 该模型累计 tokens（input + output + cache）
    pub total_tokens: u64,
}

impl CostAccumulator {
    /// 将一次 LLM 调用的使用量和模型名加入累计。
    pub fn add_response(&mut self, model: &str, usage: &CanonicalUsage) {
        self.total_input_tokens += usage.input_tokens;
        self.total_output_tokens += usage.output_tokens;
        self.total_cache_read_tokens += usage.cache_read_tokens;
        self.total_cache_write_tokens += usage.cache_write_tokens;

        let result = estimate_cost(usage, model);
        if result.pricing_known {
            self.total_input_cost_usd += result.input_cost_usd;
            self.total_output_cost_usd += result.output_cost_usd;
            self.total_cache_read_cost_usd += result.cache_read_cost_usd;
            self.total_cache_write_cost_usd += result.cache_write_cost_usd;
            self.total_cost_usd += result.total_cost_usd;
            let entry = self.per_model_costs.entry(model.to_string()).or_default();
            entry.cost_usd += result.total_cost_usd;
            entry.total_tokens += usage.input_tokens
                + usage.output_tokens
                + usage.cache_read_tokens
                + usage.cache_write_tokens;
        } else {
            let total = usage.input_tokens
                + usage.output_tokens
                + usage.cache_read_tokens
                + usage.cache_write_tokens;
            *self.unknown_models.entry(model.to_string()).or_default() += total;
        }
    }

    /// 生成 `/usage` 命令的费用展示字符串。
    pub fn summary(&self) -> String {
        if self.total_input_tokens == 0
            && self.total_output_tokens == 0
            && self.unknown_models.is_empty()
        {
            return "[usage] Cost: no LLM calls recorded yet".to_string();
        }

        let mut lines = Vec::new();

        if !self.unknown_models.is_empty() {
            for (model, tokens) in &self.unknown_models {
                lines.push(format!(
                    "[usage] Cost: pricing unknown for model {} ({} tokens recorded)",
                    model, tokens
                ));
            }
        }

        if self.total_input_tokens > 0 || self.total_output_tokens > 0 {
            lines.push(format!(
                "[usage] Cost (USD):\n  Input:        ${:.4}  ({} tokens)\n  Output:       ${:.4}  ({} tokens)\n  Cache read:   ${:.4}  ({} tokens)\n  Cache write:  ${:.4}  ({} tokens)\n  Total:        ${:.4}",
                self.total_input_cost_usd,
                self.total_input_tokens,
                self.total_output_cost_usd,
                self.total_output_tokens,
                self.total_cache_read_cost_usd,
                self.total_cache_read_tokens,
                self.total_cache_write_cost_usd,
                self.total_cache_write_tokens,
                self.total_cost_usd,
            ));

            if !self.per_model_costs.is_empty() {
                // 按费用降序排列
                let mut model_list: Vec<(&String, &ModelCost)> =
                    self.per_model_costs.iter().collect();
                model_list.sort_by(|a, b| {
                    b.1.cost_usd
                        .partial_cmp(&a.1.cost_usd)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });

                let mut by_model = "  By model:".to_string();
                for (model, mc) in model_list {
                    by_model.push_str(&format!(
                        "\n    {:40}  ${:.4}  ({} tokens)",
                        model, mc.cost_usd, mc.total_tokens
                    ));
                }
                lines.push(by_model);
            }
        }

        lines.join("\n")
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookup_claude_3_5_sonnet_returns_pricing() {
        let entry = lookup_pricing("claude-3-5-sonnet-20241022");
        assert!(
            entry.is_some(),
            "claude-3-5-sonnet-20241022 should be in pricing table"
        );
        let e = entry.unwrap();
        assert!((e.input_per_million_usd - 3.00).abs() < 1e-9);
        assert!((e.output_per_million_usd - 15.00).abs() < 1e-9);
    }

    #[test]
    fn test_lookup_unknown_model_returns_none() {
        let entry = lookup_pricing("my-custom-model-v99");
        assert!(entry.is_none(), "unknown model should return None");
    }

    #[test]
    fn test_estimate_cost_includes_all_components() {
        let usage = CanonicalUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_read_tokens: 1_000_000,
            cache_write_tokens: 1_000_000,
        };
        let result = estimate_cost(&usage, "claude-3-5-sonnet-20241022");
        assert!(result.pricing_known);
        // input: $3.00, output: $15.00, cache_read: $0.30, cache_write: $3.75
        assert!((result.input_cost_usd - 3.00).abs() < 1e-6);
        assert!((result.output_cost_usd - 15.00).abs() < 1e-6);
        assert!((result.cache_read_cost_usd - 0.30).abs() < 1e-6);
        assert!((result.cache_write_cost_usd - 3.75).abs() < 1e-6);
        assert!((result.total_cost_usd - 22.05).abs() < 1e-6);
    }

    #[test]
    fn test_estimate_cost_zero_for_unknown_model() {
        let usage = CanonicalUsage {
            input_tokens: 100_000,
            output_tokens: 50_000,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        let result = estimate_cost(&usage, "unknown-model-xyz");
        assert!(!result.pricing_known);
        assert!((result.total_cost_usd - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_accumulator_aggregates_multiple_responses() {
        let mut acc = CostAccumulator::default();
        let usage1 = CanonicalUsage {
            input_tokens: 1_000_000,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        // claude-3-5-sonnet: $3/M input
        acc.add_response("claude-3-5-sonnet-20241022", &usage1);
        assert!((acc.total_cost_usd - 3.00).abs() < 1e-6);
        assert_eq!(acc.total_input_tokens, 1_000_000);

        let usage2 = CanonicalUsage {
            input_tokens: 0,
            output_tokens: 1_000_000,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        // gpt-4o: $10/M output
        acc.add_response("gpt-4o", &usage2);
        assert!((acc.total_cost_usd - 13.00).abs() < 1e-6);
        assert_eq!(acc.total_output_tokens, 1_000_000);
    }

    #[test]
    fn test_accumulator_tracks_per_model_breakdown() {
        let mut acc = CostAccumulator::default();
        let usage = CanonicalUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        acc.add_response("claude-3-5-sonnet-20241022", &usage.clone());
        acc.add_response("gpt-4o", &usage);

        assert_eq!(acc.per_model_costs.len(), 2);
        assert!(acc
            .per_model_costs
            .contains_key("claude-3-5-sonnet-20241022"));
        assert!(acc.per_model_costs.contains_key("gpt-4o"));

        // claude-3-5-sonnet: $3 input + $15 output = $18
        let sonnet = &acc.per_model_costs["claude-3-5-sonnet-20241022"];
        assert!((sonnet.cost_usd - 18.00).abs() < 1e-6);

        // gpt-4o: $2.50 input + $10 output = $12.50
        let gpt4o = &acc.per_model_costs["gpt-4o"];
        assert!((gpt4o.cost_usd - 12.50).abs() < 1e-6);
    }

    #[test]
    fn test_prefix_matching_finds_versioned_claude() {
        // A future claude-3-5-sonnet-20260101 should still match via prefix
        let entry = lookup_pricing("claude-3-5-sonnet-20260101");
        assert!(entry.is_some());
        let e = entry.unwrap();
        assert!((e.input_per_million_usd - 3.00).abs() < 1e-9);
    }

    // BUG-CB-04: MiniMax-M2.7-HighSpeed and similar variants with a suffix after
    // "minimax-m2.7" were not matched by the exact "minimax-m2.7" entry in
    // PRICING_TABLE. The new "minimax-m2" prefix in PREFIX_TABLE covers them.
    #[test]
    fn test_lookup_minimax_m2_7_highspeed_finds_pricing() {
        let entry = lookup_pricing("MiniMax-M2.7-HighSpeed");
        assert!(
            entry.is_some(),
            "MiniMax-M2.7-HighSpeed should resolve via minimax-m2 prefix"
        );
        let e = entry.unwrap();
        assert!((e.input_per_million_usd - 0.30).abs() < 1e-9);
        assert!((e.output_per_million_usd - 1.20).abs() < 1e-9);
    }

    #[test]
    fn test_lookup_minimax_versioned_finds_pricing() {
        // Exact model name as returned by the MiniMax API, with mixed case.
        let entry = lookup_pricing("MiniMax-M2.7");
        assert!(
            entry.is_some(),
            "MiniMax-M2.7 should resolve (exact match lowercased to minimax-m2.7)"
        );
        let e = entry.unwrap();
        assert!((e.input_per_million_usd - 0.30).abs() < 1e-9);
        assert!((e.output_per_million_usd - 1.20).abs() < 1e-9);
    }
}
