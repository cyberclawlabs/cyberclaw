//! In-memory rolling usage counters for /api/v1/usage.
//!
//! Tracks per-model request counts, token usage, and first-token latency.
//! All state is in-memory and resets on server restart (V1 scope).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Per-model usage accumulator.
#[derive(Debug, Clone, Default)]
struct ModelBucket {
    requests: u64,
    input_tokens: u64,
    output_tokens: u64,
    /// Cumulative first-token latency in ms (for average calculation).
    first_token_ms_sum: u64,
    first_token_ms_count: u64,
    last_used_at: Option<String>,
}

/// Snapshot of a single model's usage (JSON-serialisable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsageSnapshot {
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Average first-token latency in ms, or 0 when unavailable.
    pub first_token_avg_ms: u64,
    pub last_used_at: Option<String>,
}

/// Top-level usage response returned by GET /api/v1/usage.
#[derive(Debug, Serialize, Deserialize)]
pub struct UsageResponse {
    pub by_model: HashMap<String, ModelUsageSnapshot>,
    pub total_requests: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_sessions: u64,
    pub estimated_cost_usd: f64,
    /// Nominal window — always 86400 (last 24 h) in this V1 in-memory impl.
    pub window_secs: u64,
}

/// Thread-safe in-memory usage store.
///
/// Designed for cheap writes on the hot path (one `Mutex` lock per completed
/// request) and cheap reads (snapshot builds in O(n) over distinct models).
pub struct UsageCounters {
    inner: Mutex<HashMap<String, ModelBucket>>,
    sessions: AtomicU64,
}

impl Default for UsageCounters {
    fn default() -> Self {
        Self::new()
    }
}

impl UsageCounters {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            sessions: AtomicU64::new(0),
        }
    }

    /// Record a completed chat request for `model`.
    ///
    /// `first_token_ms` may be 0 when first-token timing is unavailable
    /// (non-streaming path).
    pub fn record(&self, model: &str, input_tokens: u64, output_tokens: u64, first_token_ms: u64) {
        let now = chrono::Utc::now().to_rfc3339();
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let bucket = guard.entry(model.to_string()).or_default();
        bucket.requests += 1;
        bucket.input_tokens += input_tokens;
        bucket.output_tokens += output_tokens;
        if first_token_ms > 0 {
            bucket.first_token_ms_sum += first_token_ms;
            bucket.first_token_ms_count += 1;
        }
        bucket.last_used_at = Some(now);
    }

    /// Increment the distinct-session counter (call once per /chat/completions hit).
    pub fn record_session(&self) {
        self.sessions.fetch_add(1, Ordering::Relaxed);
    }

    /// Build a snapshot suitable for JSON serialisation.
    pub fn snapshot(&self) -> UsageResponse {
        let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());

        let mut total_requests = 0u64;
        let mut total_input = 0u64;
        let mut total_output = 0u64;
        let mut by_model = HashMap::new();

        for (model, bucket) in guard.iter() {
            total_requests += bucket.requests;
            total_input += bucket.input_tokens;
            total_output += bucket.output_tokens;

            let avg_ms = bucket
                .first_token_ms_sum
                .checked_div(bucket.first_token_ms_count)
                .unwrap_or(0);

            by_model.insert(
                model.clone(),
                ModelUsageSnapshot {
                    requests: bucket.requests,
                    input_tokens: bucket.input_tokens,
                    output_tokens: bucket.output_tokens,
                    first_token_avg_ms: avg_ms,
                    last_used_at: bucket.last_used_at.clone(),
                },
            );
        }

        let total_sessions = self.sessions.load(Ordering::Relaxed);
        let estimated_cost = estimate_cost_usd(&by_model);

        UsageResponse {
            by_model,
            total_requests,
            total_input_tokens: total_input,
            total_output_tokens: total_output,
            total_sessions,
            estimated_cost_usd: estimated_cost,
            window_secs: 86400,
        }
    }
}

/// Rough cost estimate based on well-known model pricing (USD per 1M tokens).
///
/// Unknown models default to $0 so the counter never panics.
fn estimate_cost_usd(by_model: &HashMap<String, ModelUsageSnapshot>) -> f64 {
    let mut total = 0.0f64;
    for (model, snap) in by_model {
        let (price_in, price_out) = model_pricing(model);
        total += snap.input_tokens as f64 / 1_000_000.0 * price_in;
        total += snap.output_tokens as f64 / 1_000_000.0 * price_out;
    }
    // Round to 4 decimal places.
    (total * 10_000.0).round() / 10_000.0
}

/// Returns (price_per_1M_input_tokens, price_per_1M_output_tokens) in USD.
fn model_pricing(model: &str) -> (f64, f64) {
    let m = model.to_ascii_lowercase();
    if m.contains("opus") {
        (15.0, 75.0)
    } else if m.contains("sonnet") {
        (3.0, 15.0)
    } else if m.contains("haiku") {
        (0.25, 1.25)
    } else if m.contains("gpt-4o-mini") || m.contains("gpt4o-mini") {
        (0.15, 0.6)
    } else if m.contains("gpt-4") {
        (5.0, 15.0)
    } else if m.contains("minimax") || m.contains("m2.7") {
        (1.0, 3.0)
    } else if m.contains("ollama") || m.contains("local") {
        (0.0, 0.0)
    } else {
        (1.0, 3.0) // safe default
    }
}
