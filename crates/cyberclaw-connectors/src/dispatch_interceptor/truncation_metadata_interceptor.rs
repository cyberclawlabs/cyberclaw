//! Truncation metadata interceptor (GAP-1 fix).
//!
//! Background: `tool_result_budget.rs::truncated_preview()` joins the head
//! and tail of an oversized payload with the literal string
//! `"\n...[truncated N bytes]...\n"`. The model is asked to notice this
//! marker mid-stream and reason about counts, but in practice it
//! frequently silently treats the truncated result as authoritative —
//! particularly damaging in shell-counting tasks where the model is asked
//! "how many X" against a tool result it thinks is complete.
//!
//! This interceptor scans the post-execute `result.output` for either the
//! literal marker substring or an existing `truncated: true` field (as
//! emitted by `dispatcher::execute_with_dispatch_budget` when the output
//! cap trips). On match, it injects a top-level `_meta` field WITHOUT
//! moving any sibling fields, so existing consumers that read top-level
//! keys (like `dispatcher::execute_with_dispatch_budget`'s
//! `truncated`/`max_size_bytes`/`preview`) keep working:
//!
//! ```json
//! {
//!   "_meta": {
//!     "truncated": true,
//!     "truncation_marker_present": true
//!   },
//!   ...original fields...
//! }
//! ```
//!
//! For non-object outputs (string/array), the interceptor wraps the value
//! under `original` since we can't add a sibling key to a non-object.
//!
//! The `_meta` key is reserved at the top level. Downstream prompts can
//! check `output._meta.truncated` deterministically instead of regex-ing
//! for the marker string.

use super::{DispatchCtx, DispatchInterceptor};

/// String marker emitted by `tool_result_budget::truncated_preview`.
const TRUNCATION_MARKER_SUBSTRING: &str = "...[truncated";

/// Stateless annotation interceptor. Safe to share via `Arc`.
#[derive(Debug, Default, Clone)]
pub struct TruncationMetadataInterceptor;

impl TruncationMetadataInterceptor {
    /// Build a new truncation-metadata interceptor.
    pub fn new() -> Self {
        Self
    }

    /// Inspect `output` for a truncation signal — either the literal marker
    /// substring or a `truncated: true` field at the top level of an object.
    fn detect_truncation(output: &serde_json::Value) -> (bool, bool) {
        // Field signal: dispatcher cap path emits `truncated: true`.
        let field_signal = output
            .as_object()
            .and_then(|m| m.get("truncated"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // String signal: tool_result_budget preview marker. We serialize
        // once and substring-match — cheaper than recursing JSON. This is
        // a single read pass over the result so it stays bounded by the
        // dispatcher's output cap (default 256 KiB).
        let serialized = serde_json::to_string(output).unwrap_or_default();
        let marker_signal = serialized.contains(TRUNCATION_MARKER_SUBSTRING);

        (field_signal, marker_signal)
    }
}

#[async_trait::async_trait]
impl DispatchInterceptor for TruncationMetadataInterceptor {
    async fn before(&self, _ctx: &mut DispatchCtx) -> anyhow::Result<()> {
        Ok(())
    }

    async fn after(
        &self,
        _ctx: &DispatchCtx,
        result: &mut crate::types::CapabilityExecutionResult,
    ) {
        let (field_signal, marker_signal) = Self::detect_truncation(&result.output);
        if !field_signal && !marker_signal {
            return;
        }

        let meta = serde_json::json!({
            "truncated": true,
            "truncation_marker_present": marker_signal,
        });

        match result.output.as_object_mut() {
            Some(map) => {
                // Object output (most common): inject `_meta` as a sibling
                // so existing top-level fields (`truncated`, `max_size_bytes`,
                // `preview`, `stdout`, …) keep their place.
                map.insert("_meta".to_string(), meta);
            }
            None => {
                // Non-object output (rare): wrap under `original` since we
                // can't add a sibling key to a string/array/scalar.
                let original = std::mem::replace(&mut result.output, serde_json::Value::Null);
                result.output = serde_json::json!({
                    "_meta": meta,
                    "original": original,
                });
            }
        }
    }
}
