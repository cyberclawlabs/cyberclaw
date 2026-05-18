//! Signal extractor — the I-Signal invariant.
//!
//! Reads a [`HistoryAnalysis`] plus optional execution-log text and user
//! input, emits a deduplicated [`Vec<String>`] of signals suitable for
//! [`crate::signal_router::route`].
//!
//! Mirrors Evolver's layered extraction at `signals.js:518-654` but stripped
//! to the regex + meta-signal layers that don't require an external LLM
//! (LLM-based Layer 3 must flow through a CyberClaw Capability when added
//! later — see CLAUDE.md §3).
//!
//! # Layers
//!
//! 1. **Regex layer** — scan free-form text for known error/perf/request
//!    patterns.
//! 2. **Meta layer** — derive synthesis signals from [`HistoryAnalysis`]:
//!    - `evolution_stagnation_detected` when consecutive empty cycles ≥ N
//!    - `consecutive_failures` / `failure_loop_detected` on failure streaks
//!    - `force_innovation_after_repair_loop` when repair runs saturate
//!
//! Suppression is **not** applied inside the extractor — this is the
//! router's job (and can be debugged independently).

use regex::Regex;

use crate::history_analyzer::HistoryAnalysis;

/// A single regex-profile rule: any match emits `signal_name`.
pub struct RegexProfile {
    pub signal_name: String,
    pub regex: Regex,
}

/// Thresholds and profiles driving [`extract_signals`]. Defaults mirror
/// Evolver's `signals.js` tuning (see field-level docs).
pub struct ExtractorConfig {
    /// Regex rules applied to `execution_log` + `user_input` combined.
    pub regex_profiles: Vec<RegexProfile>,
    /// Empty-cycle count at which to emit `evolution_stagnation_detected`.
    /// Evolver `signals.js:101-111` uses ≥ 2.
    pub stagnation_threshold: u32,
    /// Failure streak at which to emit `consecutive_failures`.
    /// Evolver `signals.js:116-123` uses ≥ 3.
    pub failure_streak_threshold: u32,
    /// Failure streak at which to escalate to `failure_loop_detected`.
    /// Evolver `signals.js:606-622` uses ≥ 5.
    pub failure_loop_threshold: u32,
    /// Repair streak at which to emit `force_innovation_after_repair_loop`.
    /// Evolver uses ≥ 3 consecutive repair intents.
    pub repair_loop_threshold: u32,
}

impl Default for ExtractorConfig {
    fn default() -> Self {
        // Regex compilation only runs once per Default call. If any pattern
        // fails to compile we panic — the patterns are constants so this is
        // a programmer error, not user error.
        let regex_profiles = vec![
            RegexProfile {
                signal_name: "error".into(),
                regex: Regex::new(r"(?i)\b(error|exception|panic|traceback|stack trace)\b")
                    .expect("static regex"),
            },
            RegexProfile {
                signal_name: "failed".into(),
                regex: Regex::new(r"(?i)\b(failed|failure|crashed)\b").expect("static regex"),
            },
            RegexProfile {
                signal_name: "perf_bottleneck".into(),
                regex: Regex::new(
                    r"(?i)\b(timeout|timed out|bottleneck|high cpu|high memory|oom|out of memory|too slow)\b",
                )
                .expect("static regex"),
            },
            RegexProfile {
                signal_name: "user_feature_request".into(),
                regex: Regex::new(
                    r"(?i)\b(please add|i want|we need|new feature|implement|support for)\b",
                )
                .expect("static regex"),
            },
            RegexProfile {
                signal_name: "user_improvement_suggestion".into(),
                regex: Regex::new(r"(?i)\b(improve|refactor|streamline|simplify|optimize)\b")
                    .expect("static regex"),
            },
        ];
        Self {
            regex_profiles,
            stagnation_threshold: 2,
            failure_streak_threshold: 3,
            failure_loop_threshold: 5,
            repair_loop_threshold: 3,
        }
    }
}

/// Inputs consumed per extraction call — borrowed, no allocation.
pub struct ExtractorInput<'a> {
    pub history: &'a HistoryAnalysis,
    pub execution_log: Option<&'a str>,
    pub user_input: Option<&'a str>,
}

/// Extract signals from one input window. Output is sorted + deduplicated.
pub fn extract_signals(input: ExtractorInput<'_>, config: &ExtractorConfig) -> Vec<String> {
    let mut signals: Vec<String> = Vec::new();

    // Layer 1: regex. Always compose — allocation cost is trivial next to
    // regex scans, and the code is simpler than conditional borrowing.
    let text = format!(
        "{}\n{}",
        input.execution_log.unwrap_or(""),
        input.user_input.unwrap_or("")
    );
    if !text.trim().is_empty() {
        for profile in &config.regex_profiles {
            if profile.regex.is_match(&text) {
                signals.push(profile.signal_name.clone());
            }
        }
    }

    // Layer 2: meta signals from history.
    if input.history.consecutive_empty_cycles >= config.stagnation_threshold {
        signals.push("evolution_stagnation_detected".into());
    }
    if input.history.consecutive_failure_count >= config.failure_loop_threshold {
        signals.push("failure_loop_detected".into());
    } else if input.history.consecutive_failure_count >= config.failure_streak_threshold {
        signals.push("consecutive_failures".into());
    }
    if input.history.consecutive_repair_count >= config.repair_loop_threshold {
        signals.push("force_innovation_after_repair_loop".into());
    }

    signals.sort();
    signals.dedup();
    signals
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_history() -> HistoryAnalysis {
        HistoryAnalysis::default()
    }

    #[test]
    fn empty_input_yields_no_signals() {
        let h = empty_history();
        let out = extract_signals(
            ExtractorInput {
                history: &h,
                execution_log: None,
                user_input: None,
            },
            &ExtractorConfig::default(),
        );
        assert!(out.is_empty());
    }

    #[test]
    fn regex_layer_detects_error() {
        let h = empty_history();
        let out = extract_signals(
            ExtractorInput {
                history: &h,
                execution_log: Some("Error: connector threw an exception while dispatching"),
                user_input: None,
            },
            &ExtractorConfig::default(),
        );
        assert!(out.contains(&"error".to_string()));
    }

    #[test]
    fn regex_layer_detects_user_request_and_perf() {
        let h = empty_history();
        let out = extract_signals(
            ExtractorInput {
                history: &h,
                execution_log: None,
                user_input: Some("Please add support for bulk import — current flow is too slow."),
            },
            &ExtractorConfig::default(),
        );
        assert!(out.contains(&"user_feature_request".to_string()));
        assert!(out.contains(&"perf_bottleneck".to_string()));
    }

    #[test]
    fn stagnation_signal_fires_on_empty_cycles() {
        let mut h = empty_history();
        h.consecutive_empty_cycles = 2;
        let out = extract_signals(
            ExtractorInput {
                history: &h,
                execution_log: None,
                user_input: None,
            },
            &ExtractorConfig::default(),
        );
        assert_eq!(out, vec!["evolution_stagnation_detected".to_string()]);
    }

    #[test]
    fn failure_streak_then_loop_escalates() {
        let mut h = empty_history();
        h.consecutive_failure_count = 3;
        let out = extract_signals(
            ExtractorInput {
                history: &h,
                execution_log: None,
                user_input: None,
            },
            &ExtractorConfig::default(),
        );
        assert!(out.contains(&"consecutive_failures".to_string()));
        assert!(!out.contains(&"failure_loop_detected".to_string()));

        h.consecutive_failure_count = 5;
        let out = extract_signals(
            ExtractorInput {
                history: &h,
                execution_log: None,
                user_input: None,
            },
            &ExtractorConfig::default(),
        );
        assert!(out.contains(&"failure_loop_detected".to_string()));
        // Mutually exclusive: only the higher-severity signal fires.
        assert!(!out.contains(&"consecutive_failures".to_string()));
    }

    #[test]
    fn repair_loop_triggers_force_innovation() {
        let mut h = empty_history();
        h.consecutive_repair_count = 3;
        let out = extract_signals(
            ExtractorInput {
                history: &h,
                execution_log: None,
                user_input: None,
            },
            &ExtractorConfig::default(),
        );
        assert!(out.contains(&"force_innovation_after_repair_loop".to_string()));
    }

    #[test]
    fn output_is_sorted_and_deduped() {
        let mut h = empty_history();
        h.consecutive_empty_cycles = 2;
        h.consecutive_failure_count = 5;
        h.consecutive_repair_count = 3;
        let out = extract_signals(
            ExtractorInput {
                history: &h,
                execution_log: Some("error error error"),
                user_input: Some("failed"),
            },
            &ExtractorConfig::default(),
        );
        let mut sorted = out.clone();
        sorted.sort();
        assert_eq!(out, sorted);
        let mut dedup = out.clone();
        dedup.dedup();
        assert_eq!(out, dedup);
    }
}
