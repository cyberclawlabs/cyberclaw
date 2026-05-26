//! Typed SSE wire protocol for CyberClaw server↔CLI streaming (v1.3 WP-3).
//!
//! This crate is a **pure leaf data crate**. It contains zero `cyberclaw-*`
//! dependencies, no async runtime, and no chrono — just serde + serde_json.
//! The goal is that both server and CLI can share the same canonical `Frame`
//! enum without forcing either side to pull in the other's transitive deps.
//!
//! # Envelope
//!
//! All frames except `Done` are serialised as:
//!
//! ```json
//! {"v": 1, "type": "<variant_snake_case>", "data": {<variant_fields>}}
//! ```
//!
//! `Done` is special: it serialises to the literal `[DONE]` sentinel (no
//! envelope) to preserve OpenAI-shaped consumer backward compat.
//!
//! # Versioning
//!
//! - Server emits `v == PROTOCOL_VERSION` (currently `1`).
//! - Clients SHOULD reject frames with `v != PROTOCOL_VERSION` with a friendly
//!   "please update" message. They MAY also accept v-less legacy frames as a
//!   migration affordance (see CLI `parse_legacy_token`).
//!
//! # Size limit
//!
//! `MAX_FRAME_BYTES` (64 KiB) is a hard cap on the serialised data string.
//! Senders should truncate large fields (e.g. `ToolComplete.preview`) before
//! calling [`Frame::to_sse_data`]; the encoder returns
//! [`EncodeError::FrameTooLarge`] if the limit is exceeded.

#![deny(missing_docs)]

mod envelope;

pub use envelope::{from_sse_data, to_sse_data, DecodeError, EncodeError};

use serde::{Deserialize, Serialize};

/// Current wire protocol version. Bumped only on backward-incompatible changes.
pub const PROTOCOL_VERSION: u8 = 1;

/// Hard cap on the serialised envelope size (bytes). Senders truncate fields
/// like `ToolComplete.preview` to stay under this limit.
pub const MAX_FRAME_BYTES: usize = 65_536;

/// `[DONE]` sentinel emitted on the wire to terminate an SSE stream. Kept as a
/// literal (no envelope) for OpenAI-shaped consumer backward compat.
pub const DONE_SENTINEL: &str = "[DONE]";

// ---------------------------------------------------------------------------
// Frame enum
// ---------------------------------------------------------------------------

/// Canonical SSE frame exchanged between server and CLI.
///
/// All variants except [`Frame::Done`] are wrapped in a versioned envelope
/// (`{"v":1,"type":"…","data":{…}}`) when serialised via [`Frame::to_sse_data`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Frame {
    /// A text fragment from the assistant's final message.
    Token {
        /// The text content of this token chunk.
        content: String,
    },
    /// A tool dispatch is about to start.
    ToolStart {
        /// LLM-visible tool name (e.g. `"fs.read"`).
        tool: String,
        /// Arguments the tool was invoked with.
        args: serde_json::Value,
    },
    /// Optional progress signal emitted by long-running tools.
    ToolProgress {
        /// LLM-visible tool name.
        tool: String,
        /// Optional 0-100 progress percentage.
        percent: Option<u8>,
        /// Optional human-readable status line.
        message: Option<String>,
    },
    /// A tool dispatch completed (successfully or otherwise).
    ToolComplete {
        /// LLM-visible tool name.
        tool: String,
        /// Whether the dispatch succeeded.
        ok: bool,
        /// Truncated preview of the result or error (≤240 chars typical).
        preview: String,
        /// Wall-clock duration of the dispatch in milliseconds.
        duration_ms: u64,
    },
    /// A tool dispatch entered the governance approval queue.
    ApprovalPending {
        /// LLM-visible tool name awaiting approval.
        tool: String,
        /// Optional human-readable reason approval is required.
        reason: Option<String>,
    },
    /// Approval was granted for a previously pending tool.
    ApprovalGranted {
        /// LLM-visible tool name that was approved.
        tool: String,
    },
    /// Approval was denied for a previously pending tool.
    ApprovalDenied {
        /// LLM-visible tool name that was denied.
        tool: String,
        /// Optional human-readable reason.
        reason: Option<String>,
    },
    /// Terminal error frame. The stream is closed after this frame (followed
    /// by `Done`).
    Error {
        /// Human-readable error message.
        message: String,
        /// Typed error category for client-side rendering.
        kind: ErrorKind,
    },
    /// Token usage snapshot for client-side cost estimation.
    Usage {
        /// LLM model name used for this session.
        model: String,
        /// Input tokens consumed (excludes cache tokens).
        input_tokens: u64,
        /// Output tokens generated.
        output_tokens: u64,
        /// Cache read tokens (Anthropic-style).
        cache_read_tokens: u64,
        /// Cache write tokens (Anthropic-style).
        cache_write_tokens: u64,
    },
    /// Rate-limit snapshot from the last LLM call.
    RateLimit {
        /// Provider name (e.g. `"anthropic"`, `"openai"`).
        provider: String,
        /// Total requests allowed in the current window.
        requests_limit: Option<u64>,
        /// Requests remaining in the current window.
        requests_remaining: Option<u64>,
        /// Total tokens allowed in the current window.
        tokens_limit: Option<u64>,
        /// Tokens remaining in the current window.
        tokens_remaining: Option<u64>,
        /// Seconds until the requests window resets.
        requests_reset_secs: Option<f64>,
        /// Seconds until the tokens window resets.
        tokens_reset_secs: Option<f64>,
    },
    /// Periodic application-level keep-alive frame. Distinct from axum's SSE
    /// comment KeepAlive (both are emitted: comment for proxies, typed frame
    /// for the TUI status bar).
    Heartbeat {
        /// Seconds elapsed since the stream started.
        elapsed_secs: u64,
    },
    /// Terminal sentinel. Serialises to the literal `[DONE]` string (no
    /// envelope) for OpenAI-shaped consumer backward compat.
    Done,
}

impl Frame {
    /// Serialise this frame into the body of an SSE `data:` line.
    ///
    /// Returns [`EncodeError::FrameTooLarge`] if the serialised envelope
    /// exceeds [`MAX_FRAME_BYTES`].
    pub fn to_sse_data(&self) -> Result<String, EncodeError> {
        to_sse_data(self)
    }

    /// Parse a single SSE `data:` body into a [`Frame`].
    pub fn from_sse_data(data: &str) -> Result<Self, DecodeError> {
        from_sse_data(data)
    }
}

// ---------------------------------------------------------------------------
// ErrorKind enum
// ---------------------------------------------------------------------------

/// Typed category for [`Frame::Error`].
///
/// The first 15 variants mirror `cyberclaw_llm::LlmFailoverReason` 1:1 so the
/// server can translate without a lossy round-trip. The last three variants
/// (`InvalidRequest`, `GovernanceDenied`, `Unknown`) cover server-side error
/// categories that don't originate from an LLM API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)] // Variant names are self-describing.
pub enum ErrorKind {
    // — LlmFailoverReason mirrors —
    Billing,
    RateLimit,
    ContextOverflow,
    ImageTooLarge,
    ModelNotFound,
    AuthInvalid,
    AuthExpired,
    PermissionDenied,
    QuotaExceeded,
    ServiceUnavailable,
    Timeout,
    InternalError,
    BadRequest,
    ContentFilter,
    ThinkingSignature,
    // — Server-side categories —
    InvalidRequest,
    GovernanceDenied,
    Unknown,
}

#[cfg(test)]
mod tests;
