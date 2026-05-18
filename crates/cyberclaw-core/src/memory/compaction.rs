//! Memory compaction strategies - lightweight, in-memory, no external LLM calls
//!
//! This module provides synchronous, hot-path compaction for Working, Episodic, and Procedural memories.
//! Design constraints:
//! - No external API calls (no LLM)
//! - In-memory only
//! - Simple algorithms (time-window, LRU, deduplication)
//! - Target: p95 < 100ms

use crate::memory_context::{ProceduralDocument, WorkingMemoryEntry};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// 压缩策略
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionStrategy {
    /// 保留最近的 N 个条目
    pub keep_recent_count: usize,
    /// 相似度去重阈值 (0.0-1.0)
    pub similarity_threshold: f32,
    /// 是否保留程序性规则
    pub keep_procedural: bool,
    /// 是否启用去重
    pub enable_deduplication: bool,
}

impl CompactionStrategy {
    /// MEDIUM #5 FIX: Create a new CompactionStrategy with parameter validation
    ///
    /// Validates:
    /// 1. keep_recent_count > 0 and <= MAX_KEEP_RECENT
    /// 2. similarity_threshold in range [0.0, 1.0] and is finite
    pub fn new(
        keep_recent_count: usize,
        similarity_threshold: f32,
        keep_procedural: bool,
        enable_deduplication: bool,
    ) -> Result<Self, String> {
        Self::validate_params(keep_recent_count, similarity_threshold)?;

        Ok(Self {
            keep_recent_count,
            similarity_threshold,
            keep_procedural,
            enable_deduplication,
        })
    }

    /// MEDIUM #5 FIX: Validate CompactionStrategy parameters
    fn validate_params(keep_recent_count: usize, similarity_threshold: f32) -> Result<(), String> {
        // 1. keep_recent_count must be > 0
        if keep_recent_count == 0 {
            return Err(format!(
                "MEDIUM #5: Invalid parameter 'keep_recent_count': {}. Must be greater than 0",
                keep_recent_count
            ));
        }

        // 2. keep_recent_count should not exceed reasonable upper limit
        const MAX_KEEP_RECENT: usize = 10000;
        if keep_recent_count > MAX_KEEP_RECENT {
            return Err(format!(
                "MEDIUM #5: Invalid parameter 'keep_recent_count': {}. Must not exceed {}",
                keep_recent_count, MAX_KEEP_RECENT
            ));
        }

        // 3. similarity_threshold must not be NaN or Infinity (check before range check)
        if !similarity_threshold.is_finite() {
            return Err(format!(
                "MEDIUM #5: Invalid parameter 'similarity_threshold': {}. Must be a finite number",
                similarity_threshold
            ));
        }

        // 4. similarity_threshold must be in range [0.0, 1.0]
        if !(0.0..=1.0).contains(&similarity_threshold) {
            return Err(format!(
                "MEDIUM #5: Invalid parameter 'similarity_threshold': {}. Must be in range [0.0, 1.0]",
                similarity_threshold
            ));
        }

        Ok(())
    }
}

impl Default for CompactionStrategy {
    fn default() -> Self {
        Self {
            keep_recent_count: 50,
            similarity_threshold: 0.85,
            keep_procedural: true,
            enable_deduplication: true,
        }
    }
}

/// 压缩检查点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionCheckpoint {
    pub working_entries: Vec<WorkingMemoryEntry>,
    pub episodic_summary: Vec<String>,
    pub procedural_docs: Vec<ProceduralDocument>,
    pub timestamp: String,
}

/// 压缩结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionResult {
    pub items_before: usize,
    pub items_after: usize,
    pub chars_saved: usize,
    pub success: bool,
    /// Error message if compaction failed (Some when success=false)
    pub error: Option<String>,
}

/// 简单的基于时间窗口和 LRU 的压缩器
pub struct SimpleCompactor;

impl SimpleCompactor {
    /// 压缩工作记忆条目
    ///
    /// 使用 LRU 策略保留最近的条目，并进行简单去重
    pub fn compact_working(
        entries: Vec<WorkingMemoryEntry>,
        strategy: &CompactionStrategy,
    ) -> Vec<WorkingMemoryEntry> {
        let result = if entries.len() <= strategy.keep_recent_count {
            entries
        } else {
            let mut result: Vec<WorkingMemoryEntry> = entries
                .into_iter()
                .rev() // 最新的优先
                .take(strategy.keep_recent_count)
                .collect();
            result.reverse(); // 恢复顺序
            result
        };

        if strategy.enable_deduplication {
            Self::deduplicate_working(result)
        } else {
            result
        }
    }

    /// 压缩情景记忆摘要
    ///
    /// 保留最近的摘要，进行去重
    pub fn compact_episodic(summaries: Vec<String>, strategy: &CompactionStrategy) -> Vec<String> {
        let result = if summaries.len() <= strategy.keep_recent_count {
            summaries
        } else {
            let mut result: Vec<String> = summaries
                .into_iter()
                .rev()
                .take(strategy.keep_recent_count)
                .collect();
            result.reverse();
            result
        };

        if strategy.enable_deduplication {
            Self::deduplicate_strings(result)
        } else {
            result
        }
    }

    /// 压缩程序性文档
    ///
    /// 如果 keep_procedural=true，保留所有；否则保留最新的 N 个
    pub fn compact_procedural(
        docs: Vec<ProceduralDocument>,
        strategy: &CompactionStrategy,
    ) -> Vec<ProceduralDocument> {
        if strategy.keep_procedural {
            return docs;
        }

        if docs.len() <= strategy.keep_recent_count {
            return docs;
        }

        docs.into_iter()
            .rev()
            .take(strategy.keep_recent_count)
            .rev()
            .collect()
    }

    /// 工作记忆去重（基于 summary 内容）
    fn deduplicate_working(entries: Vec<WorkingMemoryEntry>) -> Vec<WorkingMemoryEntry> {
        let mut seen = HashSet::new();
        entries
            .into_iter()
            .filter(|entry| {
                let key = Self::normalize_string(&entry.summary);
                seen.insert(key)
            })
            .collect()
    }

    /// 字符串去重
    fn deduplicate_strings(strings: Vec<String>) -> Vec<String> {
        let mut seen = HashSet::new();
        strings
            .into_iter()
            .filter(|s| {
                let key = Self::normalize_string(s);
                seen.insert(key)
            })
            .collect()
    }

    /// 规范化字符串用于去重（去除空格、转小写）
    fn normalize_string(s: &str) -> String {
        s.trim()
            .to_lowercase()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    }

    /// 计算压缩节省的字符数
    pub fn calculate_chars_saved(
        before: &[WorkingMemoryEntry],
        after: &[WorkingMemoryEntry],
        summaries_before: &[String],
        summaries_after: &[String],
    ) -> usize {
        let working_before: usize = before.iter().map(|e| e.summary.len()).sum();
        let working_after: usize = after.iter().map(|e| e.summary.len()).sum();

        let episodic_before: usize = summaries_before.iter().map(|s| s.len()).sum();
        let episodic_after: usize = summaries_after.iter().map(|s| s.len()).sum();

        (working_before + episodic_before).saturating_sub(working_after + episodic_after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_context::WorkingEntryKind;

    fn make_working_entry(summary: &str) -> WorkingMemoryEntry {
        WorkingMemoryEntry {
            execution_id: None,
            kind: WorkingEntryKind::ToolResult,
            summary: summary.to_string(),
            artifact_refs: vec![],
            trace_id: None,
            encrypted: false,
        }
    }

    #[test]
    fn test_compact_working_within_limit() {
        let entries = vec![make_working_entry("Entry 1"), make_working_entry("Entry 2")];

        let strategy = CompactionStrategy {
            keep_recent_count: 10,
            ..Default::default()
        };

        let result = SimpleCompactor::compact_working(entries.clone(), &strategy);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_compact_working_exceeds_limit() {
        let entries = vec![
            make_working_entry("Entry 1"),
            make_working_entry("Entry 2"),
            make_working_entry("Entry 3"),
            make_working_entry("Entry 4"),
            make_working_entry("Entry 5"),
        ];

        let strategy = CompactionStrategy {
            keep_recent_count: 3,
            enable_deduplication: false,
            ..Default::default()
        };

        let result = SimpleCompactor::compact_working(entries, &strategy);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].summary, "Entry 3"); // 保留最新的 3 个
        assert_eq!(result[2].summary, "Entry 5");
    }

    #[test]
    fn test_deduplicate_working() {
        let entries = vec![
            make_working_entry("Test entry"),
            make_working_entry("Test entry"), // 完全重复
            make_working_entry("Another entry"),
        ];

        let strategy = CompactionStrategy {
            keep_recent_count: 10,
            enable_deduplication: true,
            ..Default::default()
        };

        let result = SimpleCompactor::compact_working(entries, &strategy);
        // 去重后应该只有 2 个唯一的条目
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].summary, "Test entry");
        assert_eq!(result[1].summary, "Another entry");
    }

    #[test]
    fn test_compact_episodic() {
        let summaries = vec![
            "Summary 1".to_string(),
            "Summary 2".to_string(),
            "Summary 3".to_string(),
            "Summary 4".to_string(),
        ];

        let strategy = CompactionStrategy {
            keep_recent_count: 2,
            enable_deduplication: false,
            ..Default::default()
        };

        let result = SimpleCompactor::compact_episodic(summaries, &strategy);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], "Summary 3");
        assert_eq!(result[1], "Summary 4");
    }

    #[test]
    fn test_compact_procedural_keep_all() {
        let docs = vec![
            ProceduralDocument {
                source: "test".to_string(),
                title: "Doc 1".to_string(),
                content: "Content 1".to_string(),
            },
            ProceduralDocument {
                source: "test".to_string(),
                title: "Doc 2".to_string(),
                content: "Content 2".to_string(),
            },
        ];

        let strategy = CompactionStrategy {
            keep_procedural: true,
            ..Default::default()
        };

        let result = SimpleCompactor::compact_procedural(docs.clone(), &strategy);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_compact_procedural_limit() {
        let docs = vec![
            ProceduralDocument {
                source: "test".to_string(),
                title: "Doc 1".to_string(),
                content: "Content 1".to_string(),
            },
            ProceduralDocument {
                source: "test".to_string(),
                title: "Doc 2".to_string(),
                content: "Content 2".to_string(),
            },
            ProceduralDocument {
                source: "test".to_string(),
                title: "Doc 3".to_string(),
                content: "Content 3".to_string(),
            },
        ];

        let strategy = CompactionStrategy {
            keep_procedural: false,
            keep_recent_count: 2,
            ..Default::default()
        };

        let result = SimpleCompactor::compact_procedural(docs, &strategy);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].title, "Doc 2");
        assert_eq!(result[1].title, "Doc 3");
    }

    #[test]
    fn test_calculate_chars_saved() {
        let before = vec![
            make_working_entry("This is a long entry"),
            make_working_entry("Another long entry"),
        ];
        let after = vec![make_working_entry("Short")];

        let summaries_before = vec!["Long summary text".to_string()];
        let summaries_after = vec!["Short".to_string()];

        let saved = SimpleCompactor::calculate_chars_saved(
            &before,
            &after,
            &summaries_before,
            &summaries_after,
        );
        assert!(saved > 0);
    }

    #[test]
    fn test_normalize_string() {
        let s1 = "  Test  String  ";
        let s2 = "TestString";
        let s3 = "test string";

        let n1 = SimpleCompactor::normalize_string(s1);
        let n2 = SimpleCompactor::normalize_string(s2);
        let n3 = SimpleCompactor::normalize_string(s3);

        assert_eq!(n1, n2);
        assert_eq!(n2, n3);
    }

    // MEDIUM #5 FIX: Parameter validation tests
    #[test]
    fn test_compaction_strategy_new_valid() {
        let strategy = CompactionStrategy::new(10, 0.8, true, true);
        assert!(strategy.is_ok());
        let strategy = strategy.unwrap();
        assert_eq!(strategy.keep_recent_count, 10);
        assert_eq!(strategy.similarity_threshold, 0.8);
    }

    #[test]
    fn test_compaction_strategy_zero_count() {
        let strategy = CompactionStrategy::new(0, 0.8, true, true);
        assert!(strategy.is_err());
        let err = strategy.unwrap_err();
        assert!(err.contains("MEDIUM #5"));
        assert!(err.contains("keep_recent_count"));
        assert!(err.contains("Must be greater than 0"));
    }

    #[test]
    fn test_compaction_strategy_excessive_count() {
        let strategy = CompactionStrategy::new(20000, 0.8, true, true);
        assert!(strategy.is_err());
        let err = strategy.unwrap_err();
        assert!(err.contains("MEDIUM #5"));
        assert!(err.contains("Must not exceed"));
    }

    #[test]
    fn test_compaction_strategy_invalid_threshold_negative() {
        let strategy = CompactionStrategy::new(10, -0.1, true, true);
        assert!(strategy.is_err());
        let err = strategy.unwrap_err();
        assert!(err.contains("MEDIUM #5"));
        assert!(err.contains("similarity_threshold"));
        assert!(err.contains("Must be in range [0.0, 1.0]"));
    }

    #[test]
    fn test_compaction_strategy_invalid_threshold_too_high() {
        let strategy = CompactionStrategy::new(10, 1.5, true, true);
        assert!(strategy.is_err());
        let err = strategy.unwrap_err();
        assert!(err.contains("MEDIUM #5"));
        assert!(err.contains("Must be in range [0.0, 1.0]"));
    }

    #[test]
    fn test_compaction_strategy_invalid_threshold_nan() {
        let strategy = CompactionStrategy::new(10, f32::NAN, true, true);
        assert!(strategy.is_err());
        let err = strategy.unwrap_err();
        assert!(err.contains("MEDIUM #5"));
        assert!(err.contains("Must be a finite number"));
    }

    #[test]
    fn test_compaction_strategy_invalid_threshold_infinity() {
        let strategy = CompactionStrategy::new(10, f32::INFINITY, true, true);
        assert!(strategy.is_err());
        let err = strategy.unwrap_err();
        assert!(err.contains("MEDIUM #5"));
        assert!(err.contains("Must be a finite number"));
    }

    #[test]
    fn test_compaction_strategy_boundary_values() {
        // Test lower boundary
        let strategy = CompactionStrategy::new(1, 0.0, true, true);
        assert!(strategy.is_ok());

        // Test upper boundaries
        let strategy = CompactionStrategy::new(10000, 1.0, true, true);
        assert!(strategy.is_ok());
    }
}
