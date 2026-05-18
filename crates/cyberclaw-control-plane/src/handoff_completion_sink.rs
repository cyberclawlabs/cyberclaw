//! HandoffCompletionSink — control-plane callback for finalising a Handoff review.
//!
//! Sprint 22 T2.  Mirrors the `HandoffSink` (S21 T7) pattern: the server crate
//! provides a concrete implementation; control-plane holds an
//! `Option<Arc<dyn HandoffCompletionSink>>` and calls it when a
//! `ReviewTarget::Handoff` review is approved.
//!
//! The trait lives here (not in the server) because control-plane must not
//! depend on the server crate (the dependency arrow is server → control-plane).

use async_trait::async_trait;
use cyberclaw_core::ids::HandoffId;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Callback invoked by `process_review_result` when a
/// `ReviewTarget::Handoff` review is approved.
///
/// Implementors (typically a thin wrapper around `server::state::AppState`)
/// should:
/// - set `conversation.active_agent_id = req.to_agent_id`
/// - append `ChatMessage::handoff_card` to the conversation
/// - transition `HandoffQueue` status to `Accepted`
/// - emit `AdminEvent::HandoffCompleted`
///
/// The method is called inside `process_review_result` after the self-approval
/// guard.  Any error returned propagates as a hard failure back to the HTTP
/// handler.
#[async_trait]
pub trait HandoffCompletionSink: Send + Sync {
    /// Finalise acceptance of the given handoff.
    async fn finalize_accept(&self, handoff_id: &HandoffId) -> anyhow::Result<()>;
}

// ---------------------------------------------------------------------------
// No-op implementation
// ---------------------------------------------------------------------------

/// No-op implementation for use in tests and callers that do not need
/// handoff-review support.
///
/// Logs a `warn`-level message when called so missing wiring is detectable.
pub struct NoopHandoffCompletionSink;

#[async_trait]
impl HandoffCompletionSink for NoopHandoffCompletionSink {
    async fn finalize_accept(&self, _handoff_id: &HandoffId) -> anyhow::Result<()> {
        tracing::warn!(
            "NoopHandoffCompletionSink: handoff review approve called but no sink configured"
        );
        Ok(())
    }
}
