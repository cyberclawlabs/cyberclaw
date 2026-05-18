//! History analyzer — the feedback-to-signal bridge of the self-evolution loop.
//!
//! Reads a slice of [`crate::cycle_summary::EvolutionCycleSummary`] (oldest
//! first) and computes the indicators a future `SignalRouter` / daemon sleep
//! controller need:
//!
//! - **Suppression set**: signals that appeared `SUPPRESSION_THRESHOLD` or
//!   more times in the recent frequency tail (prevents repair loops).
//! - **Consecutive repair count**: saturation indicator for strategy pivot.
//! - **Consecutive failure count**: backoff trigger.
//! - **Consecutive empty cycles**: stagnation indicator.
//! - **Recent failure ratio**: sleep multiplier input.
//!
//! Mirrors Evolver's `analyzeRecentHistory` at `signals.js:37-143`.

use std::collections::{HashMap, HashSet};

use crate::cycle_summary::{CycleIntent, CycleStatus, EvolutionCycleSummary};

/// How many recent cycles to consider for intent / tail walks.
/// Evolver uses `recent.slice(-10)` at `signals.js:42`.
pub const DEFAULT_HISTORY_WINDOW: usize = 10;

/// Tail size for frequency counting. Evolver uses `recent.slice(-8)` at
/// `signals.js:57`.
pub const DEFAULT_FREQ_TAIL: usize = 8;

/// A signal appearing this many times in the freq tail is suppressed.
/// Matches Evolver `signals.js:81` (`>= 3`).
pub const SUPPRESSION_THRESHOLD: u32 = 3;

/// Output of [`analyze_recent`]. Mirrors Evolver's return shape at
/// `signals.js:132-143`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HistoryAnalysis {
    /// Signals over-processed in recent history. The SignalRouter should
    /// suppress these unless an escalation rule overrides.
    pub suppressed_signals: HashSet<String>,
    /// Normalized signal frequency map over the freq tail.
    pub signal_freq: HashMap<String, u32>,
    /// Intents of recent cycles (oldest→newest, bounded by window).
    pub recent_intents: Vec<CycleIntent>,
    /// Consecutive repair-intent runs at the tail.
    pub consecutive_repair_count: u32,
    /// Consecutive failed cycles at the tail.
    pub consecutive_failure_count: u32,
    /// Consecutive empty cycles (`blast_radius.is_empty()` or `meta.empty_cycle`).
    pub consecutive_empty_cycles: u32,
    /// Failed / total within the freq tail, `[0.0, 1.0]`.
    pub recent_failure_ratio: f32,
}

/// Analyze the tail of the cycle history. Input is expected oldest-first
/// (the natural order in a JSONL file). `window` bounds how far back to look.
pub fn analyze_recent(events: &[EvolutionCycleSummary], window: usize) -> HistoryAnalysis {
    if events.is_empty() || window == 0 {
        return HistoryAnalysis::default();
    }

    let start = events.len().saturating_sub(window);
    let recent = &events[start..];

    // Consecutive repair intent at tail (Evolver signals.js:45-52).
    let mut consecutive_repair_count: u32 = 0;
    for e in recent.iter().rev() {
        if matches!(e.intent, CycleIntent::Repair) {
            consecutive_repair_count += 1;
        } else {
            break;
        }
    }

    // Freq tail for signal/key counting.
    let tail_start = recent.len().saturating_sub(DEFAULT_FREQ_TAIL);
    let tail = &recent[tail_start..];

    // Normalized signal frequency.
    let mut signal_freq: HashMap<String, u32> = HashMap::new();
    for e in tail {
        for sig in &e.signals {
            let key = normalize_signal_key(sig);
            *signal_freq.entry(key).or_insert(0) += 1;
        }
    }

    // Suppression set (Evolver signals.js:77-84).
    let suppressed_signals: HashSet<String> = signal_freq
        .iter()
        .filter(|(_, count)| **count >= SUPPRESSION_THRESHOLD)
        .map(|(key, _)| key.clone())
        .collect();

    // Consecutive empty cycles (Evolver signals.js:102-111).
    let mut consecutive_empty_cycles: u32 = 0;
    for e in recent.iter().rev() {
        if e.blast_radius.is_empty() || meta_flag(e, "empty_cycle") {
            consecutive_empty_cycles += 1;
        } else {
            break;
        }
    }

    // Consecutive failures (Evolver signals.js:113-123).
    let mut consecutive_failure_count: u32 = 0;
    for e in recent.iter().rev() {
        if matches!(e.outcome.status, CycleStatus::Failed) {
            consecutive_failure_count += 1;
        } else {
            break;
        }
    }

    // Failure ratio across freq tail.
    let recent_failure_count = tail
        .iter()
        .filter(|e| matches!(e.outcome.status, CycleStatus::Failed))
        .count();
    let recent_failure_ratio = if tail.is_empty() {
        0.0
    } else {
        recent_failure_count as f32 / tail.len() as f32
    };

    let recent_intents: Vec<CycleIntent> = recent.iter().map(|e| e.intent).collect();

    HistoryAnalysis {
        suppressed_signals,
        signal_freq,
        recent_intents,
        consecutive_repair_count,
        consecutive_failure_count,
        consecutive_empty_cycles,
        recent_failure_ratio,
    }
}

/// Strip `:detail` suffixes for frequency bucketing so `errsig:divzero`
/// and `errsig:null_ref` both count as `errsig`. Evolver `signals.js:64-68`.
fn normalize_signal_key(raw: &str) -> String {
    const PREFIXES: &[&str] = &[
        "errsig",
        "recurring_errsig",
        "user_feature_request",
        "user_improvement_suggestion",
    ];
    for prefix in PREFIXES {
        if let Some(rest) = raw.strip_prefix(prefix) {
            if rest.starts_with(':') {
                return (*prefix).to_string();
            }
        }
    }
    raw.to_string()
}

/// Helper — check a bool flag in the summary's meta map.
fn meta_flag(summary: &EvolutionCycleSummary, key: &str) -> bool {
    summary
        .meta
        .get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cycle_summary::{BlastRadius, CycleOutcome};
    use chrono::Utc;

    fn mk(
        intent: CycleIntent,
        signals: &[&str],
        status: CycleStatus,
        files: u32,
    ) -> EvolutionCycleSummary {
        EvolutionCycleSummary {
            id: "test".into(),
            timestamp: Utc::now(),
            intent,
            signals: signals.iter().map(|s| (*s).to_string()).collect(),
            outcome: CycleOutcome {
                status,
                score: None,
                error: None,
            },
            blast_radius: BlastRadius {
                files,
                lines: files * 10,
            },
            variant_id: None,
            genes_used: Vec::new(),
            meta: serde_json::Map::new(),
        }
    }

    #[test]
    fn empty_history_returns_default() {
        assert_eq!(analyze_recent(&[], 10), HistoryAnalysis::default());
    }

    #[test]
    fn suppresses_signal_at_three_occurrences() {
        // "spam" appears in all 8 tail cycles, "error" in first 3.
        let events: Vec<_> = (0..8)
            .map(|i| {
                let sigs: &[&str] = if i < 3 { &["error", "spam"] } else { &["spam"] };
                mk(CycleIntent::Repair, sigs, CycleStatus::Success, 1)
            })
            .collect();

        let a = analyze_recent(&events, 10);
        assert!(a.suppressed_signals.contains("spam"));
        assert!(a.suppressed_signals.contains("error"));
        assert_eq!(a.signal_freq.get("spam").copied(), Some(8));
        assert_eq!(a.signal_freq.get("error").copied(), Some(3));
    }

    #[test]
    fn normalizes_prefixed_signal_keys() {
        let events: Vec<_> = (0..3)
            .map(|i| {
                let sig = format!("user_feature_request:feat_{i}");
                mk(
                    CycleIntent::Innovate,
                    &[sig.as_str()],
                    CycleStatus::Success,
                    2,
                )
            })
            .collect();

        let a = analyze_recent(&events, 10);
        assert_eq!(a.signal_freq.get("user_feature_request").copied(), Some(3));
        assert!(a.suppressed_signals.contains("user_feature_request"));
    }

    #[test]
    fn counts_consecutive_repair_at_tail() {
        let events = vec![
            mk(CycleIntent::Innovate, &[], CycleStatus::Success, 5),
            mk(CycleIntent::Repair, &[], CycleStatus::Failed, 1),
            mk(CycleIntent::Repair, &[], CycleStatus::Failed, 1),
            mk(CycleIntent::Repair, &[], CycleStatus::Failed, 1),
        ];
        let a = analyze_recent(&events, 10);
        assert_eq!(a.consecutive_repair_count, 3);
    }

    #[test]
    fn counts_consecutive_empty_cycles() {
        let events = vec![
            mk(CycleIntent::Innovate, &[], CycleStatus::Success, 5),
            mk(CycleIntent::Optimize, &[], CycleStatus::Success, 0),
            mk(CycleIntent::Repair, &[], CycleStatus::Success, 0),
        ];
        let a = analyze_recent(&events, 10);
        assert_eq!(a.consecutive_empty_cycles, 2);
    }

    #[test]
    fn counts_consecutive_failures_and_ratio() {
        let events = vec![
            mk(CycleIntent::Repair, &[], CycleStatus::Success, 1),
            mk(CycleIntent::Repair, &[], CycleStatus::Failed, 1),
            mk(CycleIntent::Repair, &[], CycleStatus::Failed, 1),
        ];
        let a = analyze_recent(&events, 10);
        assert_eq!(a.consecutive_failure_count, 2);
        assert!((a.recent_failure_ratio - 2.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn window_bounds_considered() {
        // 20 events, window 5 ⇒ only last 5 counted.
        let events: Vec<_> = (0..20)
            .map(|i| {
                let status = if i >= 15 {
                    CycleStatus::Failed
                } else {
                    CycleStatus::Success
                };
                mk(CycleIntent::Optimize, &["sig_a"], status, 1)
            })
            .collect();

        let a = analyze_recent(&events, 5);
        assert_eq!(a.consecutive_failure_count, 5);
        assert!((a.recent_failure_ratio - 1.0).abs() < 1e-6);
    }

    #[test]
    fn respects_meta_empty_cycle_flag() {
        let mut last = mk(CycleIntent::Innovate, &[], CycleStatus::Success, 999);
        last.meta
            .insert("empty_cycle".to_string(), serde_json::Value::Bool(true));
        let events = vec![
            mk(CycleIntent::Optimize, &[], CycleStatus::Success, 5),
            last,
        ];
        let a = analyze_recent(&events, 10);
        assert_eq!(a.consecutive_empty_cycles, 1);
    }

    #[test]
    fn zero_window_returns_default() {
        let events = vec![mk(CycleIntent::Repair, &["x"], CycleStatus::Failed, 1)];
        assert_eq!(analyze_recent(&events, 0), HistoryAnalysis::default());
    }
}
