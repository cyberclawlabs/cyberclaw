//! # Loop Delegate
//!
//! Customizable decision hooks for the [`AgenticLoop`](crate::agentic_loop::AgenticLoop).
//!
//! A `LoopDelegate` intercepts key moments in the LLM -> tool-call -> observe
//! cycle, allowing callers to approve, modify, reject, or log each step.
//!
//! Three built-in delegates are provided:
//!
//! - [`NoOpDelegate`] — pass-through, no modification.
//! - [`AutopilotDelegate`] — always proceeds, fully autonomous.
//! - [`InteractiveDelegate`] — pauses for user approval on high-risk calls.

use async_trait::async_trait;
use cyberclaw_core::capability::RiskLevel;
use cyberclaw_llm::types::ToolCall;

use crate::agentic_loop::LoopState;

// ---------------------------------------------------------------------------
// DelegateDecision
// ---------------------------------------------------------------------------

/// Decision returned by a delegate after inspecting tool calls.
#[derive(Debug, Clone)]
pub enum DelegateDecision {
    /// Execute the (possibly modified) tool calls.
    Proceed(Vec<ToolCall>),
    /// Skip these tool calls entirely and continue the loop.
    Skip,
    /// Reject the tool calls and terminate the loop with the given reason.
    Reject(String),
    /// Pause for explicit user approval before executing these tool calls.
    RequireApproval(Vec<ToolCall>),
}

// ---------------------------------------------------------------------------
// LoopDelegate trait
// ---------------------------------------------------------------------------

/// Hook trait for customizing agentic loop behaviour at key decision points.
///
/// All methods have sensible defaults so implementors only need to override
/// the hooks they care about.
#[async_trait]
pub trait LoopDelegate: Send + Sync {
    /// Called before tool calls are dispatched.
    ///
    /// Implementations may inspect, modify, filter, or reject the calls.
    /// Return a [`DelegateDecision`] to control what happens next.
    async fn on_tool_calls(&self, calls: Vec<ToolCall>, state: &LoopState) -> DelegateDecision;

    /// Called after the LLM produces a text response.
    ///
    /// Useful for logging, metrics, or content filtering.
    async fn on_text_response(&self, text: &str, state: &LoopState);

    /// Called each iteration to decide whether the loop should continue.
    ///
    /// Return `false` to terminate the loop early (e.g. external cancel signal).
    async fn should_continue(&self, state: &LoopState) -> bool;
}

// ---------------------------------------------------------------------------
// NoOpDelegate
// ---------------------------------------------------------------------------

/// Default pass-through delegate that applies no modifications.
///
/// All tool calls proceed unchanged, text responses are ignored, and the
/// loop always continues.
#[derive(Debug, Clone, Default)]
pub struct NoOpDelegate;

#[async_trait]
impl LoopDelegate for NoOpDelegate {
    async fn on_tool_calls(&self, calls: Vec<ToolCall>, _state: &LoopState) -> DelegateDecision {
        DelegateDecision::Proceed(calls)
    }

    async fn on_text_response(&self, _text: &str, _state: &LoopState) {}

    async fn should_continue(&self, _state: &LoopState) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// AutopilotDelegate
// ---------------------------------------------------------------------------

/// Fully autonomous delegate that always approves every action.
///
/// `should_continue` always returns `true`, letting the iteration budget
/// be the sole termination mechanism.
#[derive(Debug, Clone, Default)]
pub struct AutopilotDelegate;

#[async_trait]
impl LoopDelegate for AutopilotDelegate {
    async fn on_tool_calls(&self, calls: Vec<ToolCall>, _state: &LoopState) -> DelegateDecision {
        DelegateDecision::Proceed(calls)
    }

    async fn on_text_response(&self, _text: &str, _state: &LoopState) {}

    async fn should_continue(&self, _state: &LoopState) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// InteractiveDelegate
// ---------------------------------------------------------------------------

/// Channel-based delegate that pauses for user approval on risky tool calls.
///
/// Tool calls whose name maps to a risk level at or above `risk_threshold`
/// are returned as [`DelegateDecision::RequireApproval`]. The caller can
/// then present them to the user and send the approved (or modified) calls
/// back through the `approval_tx` channel.
///
/// Tool calls below the threshold proceed automatically.
#[derive(Debug)]
pub struct InteractiveDelegate {
    /// Minimum risk level that requires explicit approval.
    pub risk_threshold: RiskLevel,
    /// Sender half — the delegate pushes calls that need approval here.
    approval_tx: tokio::sync::mpsc::Sender<Vec<ToolCall>>,
    /// Receiver half — the caller sends back approved calls here.
    approval_rx: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Vec<ToolCall>>>,
    /// Risk classifier: maps tool/capability name to its risk level.
    risk_map: std::collections::HashMap<String, RiskLevel>,
}

impl InteractiveDelegate {
    /// Create a new interactive delegate.
    ///
    /// * `risk_threshold` — calls at or above this level need approval.
    /// * `risk_map` — maps tool names to their [`RiskLevel`].
    /// * `buffer` — channel buffer size for the approval round-trip.
    ///
    /// Returns `(delegate, approval_tx, pending_rx)`:
    /// - `approval_tx`: send approved calls back to the delegate.
    /// - `pending_rx`: receive calls that are waiting for approval.
    pub fn new(
        risk_threshold: RiskLevel,
        risk_map: std::collections::HashMap<String, RiskLevel>,
        buffer: usize,
    ) -> (
        Self,
        tokio::sync::mpsc::Sender<Vec<ToolCall>>,
        tokio::sync::mpsc::Receiver<Vec<ToolCall>>,
    ) {
        // Channel for delegate -> caller (pending approval)
        let (pending_tx, pending_rx) = tokio::sync::mpsc::channel(buffer);
        // Channel for caller -> delegate (approved calls)
        let (approved_tx, approved_rx) = tokio::sync::mpsc::channel(buffer);

        let delegate = Self {
            risk_threshold,
            approval_tx: pending_tx,
            approval_rx: tokio::sync::Mutex::new(approved_rx),
            risk_map,
        };

        (delegate, approved_tx, pending_rx)
    }

    /// Determine the maximum risk level across a set of tool calls.
    fn max_risk(&self, calls: &[ToolCall]) -> RiskLevel {
        calls
            .iter()
            .filter_map(|c| self.risk_map.get(&c.function.name))
            .max()
            .cloned()
            .unwrap_or(RiskLevel::Low)
    }
}

#[async_trait]
impl LoopDelegate for InteractiveDelegate {
    async fn on_tool_calls(&self, calls: Vec<ToolCall>, _state: &LoopState) -> DelegateDecision {
        let risk = self.max_risk(&calls);
        if risk >= self.risk_threshold {
            // Send the calls out for approval and wait for the response.
            if self.approval_tx.send(calls.clone()).await.is_err() {
                return DelegateDecision::Reject(
                    "approval channel closed — cannot obtain user approval".to_string(),
                );
            }
            // Wait for the approved set to come back.
            let mut rx = self.approval_rx.lock().await;
            match rx.recv().await {
                Some(approved) => {
                    if approved.is_empty() {
                        DelegateDecision::Skip
                    } else {
                        DelegateDecision::Proceed(approved)
                    }
                }
                None => DelegateDecision::Reject(
                    "approval channel closed while waiting for response".to_string(),
                ),
            }
        } else {
            DelegateDecision::Proceed(calls)
        }
    }

    async fn on_text_response(&self, _text: &str, _state: &LoopState) {}

    async fn should_continue(&self, _state: &LoopState) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_llm::types::FunctionCall;

    fn make_tool_call(name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: format!("call_{name}"),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: args.to_string(),
            },
        }
    }

    fn make_state() -> LoopState {
        LoopState {
            messages: Vec::new(),
            iteration_count: 1,
            tokens_consumed: 100,
        }
    }

    // -- NoOpDelegate -------------------------------------------------------

    #[tokio::test]
    async fn noop_delegate_passes_through_calls() {
        let delegate = NoOpDelegate;
        let calls = vec![
            make_tool_call("file.read", "{}"),
            make_tool_call("db.query", "{}"),
        ];
        let state = make_state();

        let decision = delegate.on_tool_calls(calls.clone(), &state).await;
        match decision {
            DelegateDecision::Proceed(out) => {
                assert_eq!(out.len(), 2);
                assert_eq!(out[0].function.name, "file.read");
                assert_eq!(out[1].function.name, "db.query");
            }
            other => panic!("expected Proceed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn noop_delegate_always_continues() {
        let delegate = NoOpDelegate;
        let state = make_state();
        assert!(delegate.should_continue(&state).await);
    }

    #[tokio::test]
    async fn noop_delegate_text_response_is_noop() {
        let delegate = NoOpDelegate;
        let state = make_state();
        // Just verify it doesn't panic.
        delegate.on_text_response("hello", &state).await;
    }

    // -- AutopilotDelegate --------------------------------------------------

    #[tokio::test]
    async fn autopilot_delegate_always_proceeds() {
        let delegate = AutopilotDelegate;
        let calls = vec![make_tool_call("dangerous.delete", r#"{"force":true}"#)];
        let state = make_state();

        let decision = delegate.on_tool_calls(calls, &state).await;
        match decision {
            DelegateDecision::Proceed(out) => {
                assert_eq!(out.len(), 1);
                assert_eq!(out[0].function.name, "dangerous.delete");
            }
            other => panic!("expected Proceed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn autopilot_delegate_always_continues() {
        let delegate = AutopilotDelegate;
        let mut state = make_state();
        state.iteration_count = 999;
        state.tokens_consumed = 999_999;
        assert!(delegate.should_continue(&state).await);
    }

    // -- InteractiveDelegate ------------------------------------------------

    #[tokio::test]
    async fn interactive_delegate_low_risk_auto_proceeds() {
        let mut risk_map = std::collections::HashMap::new();
        risk_map.insert("file.read".to_string(), RiskLevel::Low);

        let (delegate, _approved_tx, _pending_rx) =
            InteractiveDelegate::new(RiskLevel::High, risk_map, 8);

        let calls = vec![make_tool_call("file.read", "{}")];
        let state = make_state();

        let decision = delegate.on_tool_calls(calls, &state).await;
        match decision {
            DelegateDecision::Proceed(out) => {
                assert_eq!(out.len(), 1);
                assert_eq!(out[0].function.name, "file.read");
            }
            other => panic!("expected Proceed for low-risk call, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn interactive_delegate_high_risk_requires_approval() {
        let mut risk_map = std::collections::HashMap::new();
        risk_map.insert("system.exec".to_string(), RiskLevel::Critical);

        let (delegate, approved_tx, mut pending_rx) =
            InteractiveDelegate::new(RiskLevel::High, risk_map, 8);

        let calls = vec![make_tool_call("system.exec", r#"{"cmd":"rm -rf /"}"#)];
        let state = make_state();

        // Spawn the delegate call in a background task.
        let handle = tokio::spawn(async move { delegate.on_tool_calls(calls, &state).await });

        // Receive the pending approval request.
        let pending = pending_rx
            .recv()
            .await
            .expect("should receive pending calls");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].function.name, "system.exec");

        // Approve the calls (send them back).
        approved_tx.send(pending).await.expect("send approved");

        // Check the decision.
        let decision = handle.await.expect("task should complete");
        match decision {
            DelegateDecision::Proceed(out) => {
                assert_eq!(out.len(), 1);
                assert_eq!(out[0].function.name, "system.exec");
            }
            other => panic!("expected Proceed after approval, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn interactive_delegate_skip_on_empty_approval() {
        let mut risk_map = std::collections::HashMap::new();
        risk_map.insert("system.exec".to_string(), RiskLevel::Critical);

        let (delegate, approved_tx, mut pending_rx) =
            InteractiveDelegate::new(RiskLevel::High, risk_map, 8);

        let calls = vec![make_tool_call("system.exec", "{}")];
        let state = make_state();

        let handle = tokio::spawn(async move { delegate.on_tool_calls(calls, &state).await });

        // Receive and send back empty vec to skip.
        let _pending = pending_rx.recv().await.expect("should receive pending");
        approved_tx.send(vec![]).await.expect("send empty");

        let decision = handle.await.expect("task should complete");
        assert!(matches!(decision, DelegateDecision::Skip));
    }

    #[tokio::test]
    async fn interactive_delegate_reject_on_closed_channel() {
        let mut risk_map = std::collections::HashMap::new();
        risk_map.insert("system.exec".to_string(), RiskLevel::Critical);

        let (delegate, approved_tx, mut pending_rx) =
            InteractiveDelegate::new(RiskLevel::High, risk_map, 8);

        let calls = vec![make_tool_call("system.exec", "{}")];
        let state = make_state();

        let handle = tokio::spawn(async move { delegate.on_tool_calls(calls, &state).await });

        // Receive the pending request, then drop the approved_tx to simulate close.
        let _pending = pending_rx.recv().await.expect("should receive pending");
        drop(approved_tx);

        let decision = handle.await.expect("task should complete");
        match decision {
            DelegateDecision::Reject(reason) => {
                assert!(reason.contains("closed"));
            }
            other => panic!("expected Reject on closed channel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn interactive_delegate_unknown_tool_defaults_to_low_risk() {
        let risk_map = std::collections::HashMap::new(); // empty map

        let (delegate, _approved_tx, _pending_rx) =
            InteractiveDelegate::new(RiskLevel::High, risk_map, 8);

        let calls = vec![make_tool_call("unknown.tool", "{}")];
        let state = make_state();

        // Unknown tool -> Low risk -> below High threshold -> auto-proceed.
        let decision = delegate.on_tool_calls(calls, &state).await;
        assert!(matches!(decision, DelegateDecision::Proceed(_)));
    }
}
