//! # Tool Result Budget
//!
//! Three-layer budget management for tool results to prevent context-window
//! overflow in agentic loops.
//!
//! 1. **Per-tool threshold** — each tool has a maximum result size. Results
//!    exceeding this limit are truncated with a preview.
//! 2. **Per-turn aggregate budget** — all tool results in a single assistant
//!    turn share a total byte budget. When the aggregate exceeds the limit,
//!    the largest results are truncated first.
//! 3. **Pinned tools** — certain tools (e.g. `read_file`, `search`) are
//!    exempt from truncation to prevent persist-read-persist loops.
//!
//! Resolution priority: `pinned_tools` > `tool_overrides` > `default_result_size`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// BudgetConfig
// ---------------------------------------------------------------------------

/// Tool result budget configuration.
///
/// Controls the three-layer defence against context overflow from tool outputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Per-tool result default maximum size in bytes. Default: 100 000.
    pub default_result_size: usize,
    /// Aggregate budget for all tool results in a single turn (bytes). Default: 200 000.
    pub turn_budget: usize,
    /// Inline preview size when a result is truncated (bytes). Default: 1 500.
    pub preview_size: usize,
    /// Per-tool size overrides. `None` means unlimited.
    pub tool_overrides: HashMap<String, Option<usize>>,
    /// Pinned tool thresholds that cannot be overridden. `None` means unlimited.
    pub pinned_tools: HashMap<String, Option<usize>>,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        let mut pinned_tools = HashMap::new();
        pinned_tools.insert("read_file".to_string(), None);
        pinned_tools.insert("search".to_string(), None);

        Self {
            default_result_size: 100_000,
            turn_budget: 200_000,
            preview_size: 1_500,
            tool_overrides: HashMap::new(),
            pinned_tools,
        }
    }
}

// ---------------------------------------------------------------------------
// BudgetedResult
// ---------------------------------------------------------------------------

/// Outcome of applying the budget to a single tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetedResult {
    /// The result fits within the budget and is returned as-is.
    Inline(String),
    /// The result exceeded the budget and was truncated.
    Truncated {
        /// Short preview of the original content.
        preview: String,
        /// Size of the full, untruncated result in bytes.
        full_size: usize,
        /// Optional reference to a persisted artifact containing the full result.
        artifact_ref: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// ToolResultBudget
// ---------------------------------------------------------------------------

/// Stateful budget manager for tool results within a single turn.
///
/// # Example
///
/// ```
/// use cyberclaw_agent_runtime::tool_result_budget::ToolResultBudget;
///
/// let mut budget = ToolResultBudget::with_defaults();
/// let result = budget.apply("my_tool", "hello world");
/// // Small result is returned inline.
/// ```
#[derive(Debug, Clone)]
pub struct ToolResultBudget {
    config: BudgetConfig,
    /// Bytes consumed in the current turn.
    turn_used: usize,
}

impl ToolResultBudget {
    /// Create a new budget manager with the given configuration.
    pub fn new(config: BudgetConfig) -> Self {
        Self {
            config,
            turn_used: 0,
        }
    }

    /// Create a budget manager with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(BudgetConfig::default())
    }

    /// Resolve the effective threshold for `tool_name`.
    ///
    /// Priority: `pinned_tools` > `tool_overrides` > `default_result_size`.
    /// Returns `None` when the tool is unlimited.
    pub fn resolve_threshold(&self, tool_name: &str) -> Option<usize> {
        if let Some(limit) = self.config.pinned_tools.get(tool_name) {
            return *limit;
        }
        if let Some(limit) = self.config.tool_overrides.get(tool_name) {
            return *limit;
        }
        Some(self.config.default_result_size)
    }

    /// Apply budget limits to a tool result.
    ///
    /// If the result fits within both the per-tool threshold and the remaining
    /// turn budget, it is returned as [`BudgetedResult::Inline`]. Otherwise it
    /// is truncated to a preview and returned as [`BudgetedResult::Truncated`].
    pub fn apply(&mut self, tool_name: &str, result: &str) -> BudgetedResult {
        let size = result.len();
        let threshold = self.resolve_threshold(tool_name);

        // Unlimited tool — always inline, still track turn usage.
        if threshold.is_none() {
            self.turn_used = self.turn_used.saturating_add(size);
            return BudgetedResult::Inline(result.to_string());
        }

        let tool_limit = threshold.unwrap(); // safe: checked above

        // Check per-tool threshold AND remaining turn budget.
        let remaining = self.remaining_budget();
        let effective_limit = tool_limit.min(remaining);

        if size <= effective_limit {
            self.turn_used = self.turn_used.saturating_add(size);
            return BudgetedResult::Inline(result.to_string());
        }

        // Result exceeds budget — generate preview.
        let preview = Self::generate_preview(result, self.config.preview_size);
        self.turn_used = self.turn_used.saturating_add(preview.len());

        BudgetedResult::Truncated {
            preview,
            full_size: size,
            artifact_ref: None,
        }
    }

    /// Reset the turn budget counter. Call this at the start of each new turn.
    pub fn reset_turn(&mut self) {
        self.turn_used = 0;
    }

    /// Return the number of bytes remaining in the current turn budget.
    pub fn remaining_budget(&self) -> usize {
        self.config.turn_budget.saturating_sub(self.turn_used)
    }

    /// Generate a truncated preview of `content`.
    ///
    /// Strategy: take the first `max_size / 2` bytes and the last `max_size / 2`
    /// bytes, joined by a truncation marker. Byte boundaries are adjusted to
    /// valid UTF-8 character boundaries.
    fn generate_preview(content: &str, max_size: usize) -> String {
        if content.len() <= max_size {
            return content.to_string();
        }

        let half = max_size / 2;
        let marker = format!("\n...[truncated {} bytes]...\n", content.len());

        // Find a safe char boundary for the head portion.
        let head_end = Self::floor_char_boundary(content, half);
        let head = &content[..head_end];

        // Find a safe char boundary for the tail portion.
        let tail_start_raw = content.len().saturating_sub(half);
        let tail_start = Self::ceil_char_boundary(content, tail_start_raw);
        let tail = &content[tail_start..];

        format!("{head}{marker}{tail}")
    }

    /// Return the largest byte index `<= pos` that lies on a char boundary.
    fn floor_char_boundary(s: &str, pos: usize) -> usize {
        let pos = pos.min(s.len());
        let mut idx = pos;
        while idx > 0 && !s.is_char_boundary(idx) {
            idx -= 1;
        }
        idx
    }

    /// Return the smallest byte index `>= pos` that lies on a char boundary.
    fn ceil_char_boundary(s: &str, pos: usize) -> usize {
        let pos = pos.min(s.len());
        let mut idx = pos;
        while idx < s.len() && !s.is_char_boundary(idx) {
            idx += 1;
        }
        idx
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = BudgetConfig::default();
        assert_eq!(cfg.default_result_size, 100_000);
        assert_eq!(cfg.turn_budget, 200_000);
        assert_eq!(cfg.preview_size, 1_500);
        assert_eq!(cfg.pinned_tools.get("read_file"), Some(&None));
        assert_eq!(cfg.pinned_tools.get("search"), Some(&None));
        assert!(cfg.tool_overrides.is_empty());
    }

    #[test]
    fn test_small_result_inline() {
        let mut budget = ToolResultBudget::with_defaults();
        let result = budget.apply("some_tool", "hello world");
        assert_eq!(result, BudgetedResult::Inline("hello world".to_string()));
    }

    #[test]
    fn test_large_result_truncated() {
        let mut budget = ToolResultBudget::new(BudgetConfig {
            default_result_size: 50,
            preview_size: 20,
            ..BudgetConfig::default()
        });

        let large = "a".repeat(200);
        let result = budget.apply("some_tool", &large);

        match result {
            BudgetedResult::Truncated {
                preview,
                full_size,
                artifact_ref,
            } => {
                assert_eq!(full_size, 200);
                assert!(preview.contains("truncated 200 bytes"));
                assert!(artifact_ref.is_none());
                // Preview should be much smaller than the original.
                assert!(preview.len() < 100);
            }
            _ => panic!("expected Truncated"),
        }
    }

    #[test]
    fn test_turn_budget_enforcement() {
        let mut budget = ToolResultBudget::new(BudgetConfig {
            default_result_size: 1_000,
            turn_budget: 100,
            preview_size: 20,
            ..BudgetConfig::default()
        });

        // First call consumes 80 bytes (within turn budget of 100).
        let first = "x".repeat(80);
        let r1 = budget.apply("tool_a", &first);
        assert!(matches!(r1, BudgetedResult::Inline(_)));
        assert_eq!(budget.remaining_budget(), 20);

        // Second call is 50 bytes but only 20 remain — should be truncated.
        let second = "y".repeat(50);
        let r2 = budget.apply("tool_b", &second);
        assert!(matches!(r2, BudgetedResult::Truncated { .. }));
    }

    #[test]
    fn test_pinned_tool_unlimited() {
        let mut budget = ToolResultBudget::new(BudgetConfig {
            default_result_size: 10,
            turn_budget: 50,
            preview_size: 5,
            ..BudgetConfig::default()
        });

        // read_file is pinned with None (unlimited).
        let huge = "z".repeat(500);
        let result = budget.apply("read_file", &huge);
        assert_eq!(result, BudgetedResult::Inline(huge));
    }

    #[test]
    fn test_tool_override() {
        let mut overrides = HashMap::new();
        overrides.insert("custom_tool".to_string(), Some(30));

        let mut budget = ToolResultBudget::new(BudgetConfig {
            default_result_size: 1_000,
            tool_overrides: overrides,
            preview_size: 10,
            ..BudgetConfig::default()
        });

        // 30 bytes fits custom_tool's override.
        let fits = "a".repeat(30);
        let r1 = budget.apply("custom_tool", &fits);
        assert!(matches!(r1, BudgetedResult::Inline(_)));

        budget.reset_turn();

        // 31 bytes exceeds custom_tool's override.
        let exceeds = "b".repeat(31);
        let r2 = budget.apply("custom_tool", &exceeds);
        assert!(matches!(r2, BudgetedResult::Truncated { .. }));
    }

    #[test]
    fn test_priority_pinned_over_override() {
        let mut overrides = HashMap::new();
        overrides.insert("read_file".to_string(), Some(10));

        let mut budget = ToolResultBudget::new(BudgetConfig {
            default_result_size: 5,
            tool_overrides: overrides,
            turn_budget: 1_000_000,
            ..BudgetConfig::default()
        });

        // Pinned (None = unlimited) should win over the override of 10.
        let big = "q".repeat(500);
        let result = budget.apply("read_file", &big);
        assert_eq!(result, BudgetedResult::Inline(big));
    }

    #[test]
    fn test_reset_turn() {
        let mut budget = ToolResultBudget::new(BudgetConfig {
            default_result_size: 1_000,
            turn_budget: 100,
            preview_size: 10,
            ..BudgetConfig::default()
        });

        let data = "a".repeat(80);
        budget.apply("tool", &data);
        assert_eq!(budget.remaining_budget(), 20);

        budget.reset_turn();
        assert_eq!(budget.remaining_budget(), 100);
    }

    #[test]
    fn test_preview_char_boundary() {
        let mut budget = ToolResultBudget::new(BudgetConfig {
            default_result_size: 10,
            preview_size: 20,
            ..BudgetConfig::default()
        });

        // Multi-byte: each char is 3 bytes.
        let content = "你好世界这是一个测试用的中文字符串需要足够长才能触发截断";
        let result = budget.apply("tool", content);

        match result {
            BudgetedResult::Truncated { preview, .. } => {
                // The preview must be valid UTF-8 (String guarantees this).
                // Verify it doesn't panic and contains the marker.
                assert!(preview.contains("truncated"));
                assert!(preview.is_char_boundary(0));
            }
            _ => panic!("expected Truncated for multi-byte content exceeding threshold"),
        }
    }

    #[test]
    fn test_override_unlimited() {
        let mut overrides = HashMap::new();
        overrides.insert("special".to_string(), None);

        let mut budget = ToolResultBudget::new(BudgetConfig {
            default_result_size: 10,
            turn_budget: 1_000_000,
            tool_overrides: overrides,
            ..BudgetConfig::default()
        });

        let big = "x".repeat(500);
        let result = budget.apply("special", &big);
        assert_eq!(result, BudgetedResult::Inline(big));
    }
}
