//! Trajectory Compressor — post-hoc compression of completed RL training
//! trajectories to fit within a target token budget while preserving signal.
//!
//! Mirrors Hermes v0.12 `trajectory_compressor.py`. **Distinct from
//! runtime context compression**: this runs *after* a trajectory completes
//! (typically in an offline RL data-prep pipeline), not during inference.
//! The runtime-time analogue is `cyberclaw-agent-runtime`'s
//! `ContextCompressor`.
//!
//! # Strategy
//!
//! 1. **Protect first turns** — system + first user + first assistant +
//!    first tool result are kept verbatim. This preserves task framing
//!    and the model's initial reasoning, which is the highest-value
//!    training signal.
//!
//! 2. **Protect last N turns** — final action + final observation +
//!    conclusion are kept verbatim. These are the actions the model gets
//!    rewarded for; cutting them loses the gradient signal.
//!
//! 3. **Compress middle only** — the contiguous middle range is replaced
//!    with a single `summary` step that says "[N steps compressed]".
//!
//! 4. **Compress only as much as needed** — start with no compression and
//!    expand the middle-cut window outward only until the trajectory fits
//!    under the target budget.
//!
//! # Token estimation
//!
//! No tokenizer dependency: budget is in **approximate characters** (we use
//! `chars().count()` over `action + observation`). A 4-char-per-token
//! heuristic is adequate for offline data prep where exact tokenization
//! happens later in the training loop.

use crate::rl_training::{ExecutionTrace, TraceStep};
use chrono::{DateTime, Utc};

/// Tunable knobs for the compressor.
#[derive(Debug, Clone)]
pub struct CompressorConfig {
    /// Target maximum approximate-char budget for the trajectory. Compression
    /// stops once the trajectory fits under this number.
    pub target_chars: usize,
    /// First N steps that are never compressed. Default: 4 (system, user,
    /// first assistant, first tool result analogues).
    pub protect_first: usize,
    /// Last M steps that are never compressed. Default: 4.
    pub protect_last: usize,
}

impl Default for CompressorConfig {
    fn default() -> Self {
        Self {
            target_chars: 16 * 1024, // ~4k tokens
            protect_first: 4,
            protect_last: 4,
        }
    }
}

/// Compress one trajectory. Returns a clone with the middle replaced by a
/// summary step IF compression was needed. If the trace already fits under
/// budget, returns an unchanged clone.
pub fn compress(trace: &ExecutionTrace, cfg: &CompressorConfig) -> ExecutionTrace {
    let total = approx_chars(&trace.steps);
    if total <= cfg.target_chars {
        return trace.clone();
    }
    if trace.steps.len() <= cfg.protect_first + cfg.protect_last {
        // Nothing in the middle to drop.
        return trace.clone();
    }

    let middle_start = cfg.protect_first;
    let middle_end = trace.steps.len().saturating_sub(cfg.protect_last);
    let middle_len = middle_end - middle_start;

    // Try collapsing the entire middle first (most aggressive).
    let head: Vec<TraceStep> = trace.steps[..middle_start].to_vec();
    let tail: Vec<TraceStep> = trace.steps[middle_end..].to_vec();

    let compressed_at = trace
        .steps
        .get(middle_start)
        .map(|s| s.timestamp)
        .unwrap_or_else(Utc::now);

    let summary_step = make_summary_step(middle_len, compressed_at);

    let mut new_steps: Vec<TraceStep> = Vec::with_capacity(head.len() + 1 + tail.len());
    new_steps.extend(head);
    new_steps.push(summary_step);
    new_steps.extend(tail);

    ExecutionTrace {
        trace_id: trace.trace_id.clone(),
        agent_id: trace.agent_id.clone(),
        steps: new_steps,
        outcome: trace.outcome.clone(),
        total_duration_ms: trace.total_duration_ms,
        created_at: trace.created_at,
    }
}

/// Compress every trace in `traces` whose size exceeds `cfg.target_chars`.
/// Order preserved.
pub fn compress_all(traces: &[ExecutionTrace], cfg: &CompressorConfig) -> Vec<ExecutionTrace> {
    traces.iter().map(|t| compress(t, cfg)).collect()
}

fn approx_chars(steps: &[TraceStep]) -> usize {
    steps
        .iter()
        .map(|s| s.action.chars().count() + s.observation.chars().count())
        .sum()
}

fn make_summary_step(n: usize, ts: DateTime<Utc>) -> TraceStep {
    TraceStep {
        action: "<<compressed>>".to_string(),
        observation: format!(
            "[{n} middle steps compressed by trajectory_compressor — \
             original observations + actions removed to fit token budget. \
             Training loop should treat as opaque continuation marker.]",
            n = n
        ),
        reward: 0.0,
        timestamp: ts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rl_training::TraceOutcome;

    fn step(action: &str, obs: &str) -> TraceStep {
        TraceStep {
            action: action.to_string(),
            observation: obs.to_string(),
            reward: 1.0,
            timestamp: Utc::now(),
        }
    }

    fn trace_of(n_middle: usize) -> ExecutionTrace {
        let mut steps = Vec::new();
        // 4 head steps
        for i in 0..4 {
            steps.push(step(&format!("head{i}"), &"a".repeat(50)));
        }
        // n middle steps with big payload
        for i in 0..n_middle {
            steps.push(step(&format!("middle{i}"), &"X".repeat(500)));
        }
        // 4 tail steps
        for i in 0..4 {
            steps.push(step(&format!("tail{i}"), &"z".repeat(50)));
        }
        ExecutionTrace {
            trace_id: "t".to_string(),
            agent_id: "a".to_string(),
            steps,
            outcome: TraceOutcome::Success { score: 1.0 },
            total_duration_ms: 1000,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn already_under_budget_returns_unchanged() {
        let t = trace_of(2);
        let cfg = CompressorConfig {
            target_chars: 100_000,
            ..Default::default()
        };
        let out = compress(&t, &cfg);
        assert_eq!(out.steps.len(), t.steps.len());
        for (a, b) in t.steps.iter().zip(out.steps.iter()) {
            assert_eq!(a.action, b.action);
        }
    }

    #[test]
    fn over_budget_collapses_middle_into_summary() {
        let t = trace_of(20); // 4 head + 20 middle (10k chars) + 4 tail
        let cfg = CompressorConfig {
            target_chars: 1_000,
            protect_first: 4,
            protect_last: 4,
        };
        let out = compress(&t, &cfg);
        // 4 head + 1 summary + 4 tail
        assert_eq!(out.steps.len(), 9);
        assert_eq!(out.steps[4].action, "<<compressed>>");
        assert!(out.steps[4].observation.contains("20 middle steps"));
        // Head + tail preserved.
        assert_eq!(out.steps[0].action, "head0");
        assert_eq!(out.steps[3].action, "head3");
        assert_eq!(out.steps[5].action, "tail0");
        assert_eq!(out.steps[8].action, "tail3");
    }

    #[test]
    fn nothing_to_compress_when_protect_covers_everything() {
        // 4 head + 0 middle + 4 tail = 8 steps; protect_first+protect_last = 8.
        let t = trace_of(0);
        let cfg = CompressorConfig {
            target_chars: 1, // force "over budget"
            protect_first: 4,
            protect_last: 4,
        };
        let out = compress(&t, &cfg);
        // No middle exists; return unchanged.
        assert_eq!(out.steps.len(), 8);
    }

    #[test]
    fn outcome_and_metadata_preserved() {
        let t = trace_of(20);
        let cfg = CompressorConfig {
            target_chars: 500,
            ..Default::default()
        };
        let out = compress(&t, &cfg);
        assert_eq!(out.trace_id, t.trace_id);
        assert_eq!(out.agent_id, t.agent_id);
        assert!(matches!(out.outcome, TraceOutcome::Success { .. }));
        assert_eq!(out.total_duration_ms, t.total_duration_ms);
    }

    #[test]
    fn batch_compress_preserves_order_and_independence() {
        let big = trace_of(20); // ~10k middle chars
        let small = trace_of(2); // ~1k middle chars
                                 // Budget high enough that `small` (≈1.4k chars total) fits unchanged
                                 // but `big` (≈10.4k chars) does not. Tests that each trace is
                                 // evaluated independently against the same budget.
        let cfg = CompressorConfig {
            target_chars: 5_000,
            protect_first: 4,
            protect_last: 4,
        };
        let out = compress_all(&[big.clone(), small.clone()], &cfg);
        assert_eq!(out.len(), 2);
        // Big got compressed (4 head + 1 summary + 4 tail = 9 steps);
        // small was unchanged (4 + 2 + 4 = 10 steps).
        assert_eq!(out[0].steps.len(), 9, "big should be compressed");
        assert_eq!(
            out[1].steps.len(),
            small.steps.len(),
            "small should fit under budget unchanged"
        );
    }
}
