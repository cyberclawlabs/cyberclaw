//! Per-conversation broadcast channel for clarify events.
//!
//! Uses `tokio::sync::broadcast` for multi-subscriber fan-out so multiple
//! SSE connections on the same conversation all receive the same events.
//!
//! # Design
//!
//! - One broadcast channel per conversation id, created lazily on first subscribe.
//! - `publish` sends to all active receivers; returns 0 when nobody is listening
//!   (that is normal — the agent loop still blocks in `ask()` regardless).
//! - `subscribe` creates the channel if absent, then returns a new `Receiver`.
//! - Channels are retained until the process restarts; subscriber drop causes
//!   the broadcast channel to self-GC via `receiver_count() == 0` checks, but
//!   we do not do explicit cleanup in the hot path. `cleanup_inactive` is
//!   provided for optional periodic housekeeping.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

use cyberclaw_core::clarify::{ClarifyAnswer, ClarifyRequest};
use cyberclaw_core::ids::ClarifyId;

// ---------------------------------------------------------------------------
// ClarifyEvent
// ---------------------------------------------------------------------------

/// Events broadcast to all SSE subscribers of a conversation.
#[derive(Debug, Clone)]
pub enum ClarifyEvent {
    /// Agent is asking the user a clarify question.
    Requested(ClarifyRequest),
    /// User has answered; agent loop is resuming.
    Resolved {
        id: ClarifyId,
        answer: ClarifyAnswer,
    },
}

// ---------------------------------------------------------------------------
// ClarifyBroadcaster
// ---------------------------------------------------------------------------

/// Fan-out broadcaster keyed by `conversation_id`.
///
/// Cheaply `Clone`-able via inner `Arc`.
#[derive(Debug, Clone)]
pub struct ClarifyBroadcaster {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    /// conversation_id → broadcast sender.
    channels: RwLock<HashMap<String, broadcast::Sender<ClarifyEvent>>>,
    channel_capacity: usize,
}

impl ClarifyBroadcaster {
    /// Create a broadcaster with a per-channel capacity of 32 slots.
    ///
    /// Clarify events are low-frequency (<< 1/s), so 32 is well above any
    /// realistic burst.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                channels: RwLock::new(HashMap::new()),
                channel_capacity: 32,
            }),
        }
    }

    /// Publish `event` to all current subscribers of `conversation_id`.
    ///
    /// Returns the number of receivers that were notified (0 if no active
    /// SSE connections exist — this is expected and not an error).
    pub async fn publish(&self, conversation_id: &str, event: ClarifyEvent) -> usize {
        let channels = self.inner.channels.read().await;
        if let Some(sender) = channels.get(conversation_id) {
            sender.send(event).unwrap_or(0)
        } else {
            0
        }
    }

    /// Subscribe to clarify events for `conversation_id`.
    ///
    /// Creates a new broadcast channel for the conversation on the first
    /// call. Subsequent calls return a new `Receiver` on the same channel.
    pub async fn subscribe(&self, conversation_id: &str) -> broadcast::Receiver<ClarifyEvent> {
        // Fast path: channel already exists.
        {
            let channels = self.inner.channels.read().await;
            if let Some(sender) = channels.get(conversation_id) {
                return sender.subscribe();
            }
        }
        // Slow path: create channel under write lock.
        let mut channels = self.inner.channels.write().await;
        let sender = channels
            .entry(conversation_id.to_string())
            .or_insert_with(|| broadcast::channel(self.inner.channel_capacity).0);
        sender.subscribe()
    }

    /// Remove channels that have no active receivers.
    ///
    /// This is an optional maintenance call; the broadcaster works correctly
    /// without ever calling it. Call from a background task if process lifetime
    /// is very long and you want to reclaim HashMap memory.
    pub async fn cleanup_inactive(&self) {
        let mut channels = self.inner.channels.write().await;
        channels.retain(|_, sender| sender.receiver_count() > 0);
    }
}

impl Default for ClarifyBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use cyberclaw_core::clarify::{ClarifyOption, ClarifyQuestion, ClarifyStatus};
    use cyberclaw_core::ids::AgentId;

    fn make_request(conv_id: &str) -> ClarifyRequest {
        let now = Utc::now();
        ClarifyRequest {
            id: ClarifyId::new(),
            conversation_id: conv_id.to_string(),
            agent_id: AgentId::from_string("test-agent".to_string()).unwrap(),
            questions: vec![ClarifyQuestion {
                question: "Which env?".to_string(),
                options: vec![
                    ClarifyOption {
                        label: "staging".to_string(),
                        description: "staging env".to_string(),
                        preview: None,
                    },
                    ClarifyOption {
                        label: "prod".to_string(),
                        description: "production env".to_string(),
                        preview: None,
                    },
                ],
                multi_select: false,
            }],
            source: None,
            created_at: now,
            expires_at: now + chrono::Duration::seconds(300),
            status: ClarifyStatus::Pending,
            answers: None,
            resolved_at: None,
        }
    }

    fn make_answer() -> ClarifyAnswer {
        let mut a = ClarifyAnswer::new();
        a.insert("Which env?", "staging");
        a
    }

    // ── subscribe before publish → receives event ─────────────────────────

    #[tokio::test]
    async fn test_subscriber_receives_requested_event() {
        let broadcaster = ClarifyBroadcaster::new();
        let mut rx = broadcaster.subscribe("conv-1").await;

        let req = make_request("conv-1");
        let id = req.id.clone();
        broadcaster
            .publish("conv-1", ClarifyEvent::Requested(req.clone()))
            .await;

        let event = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("should not timeout")
            .expect("recv ok");

        match event {
            ClarifyEvent::Requested(r) => assert_eq!(r.id, id),
            _ => panic!("expected Requested"),
        }
    }

    // ── publish with no subscribers returns 0 ─────────────────────────────

    #[tokio::test]
    async fn test_publish_no_subscribers_returns_zero() {
        let broadcaster = ClarifyBroadcaster::new();
        let count = broadcaster
            .publish(
                "conv-nobody",
                ClarifyEvent::Requested(make_request("conv-nobody")),
            )
            .await;
        assert_eq!(count, 0);
    }

    // ── resolved event arrives after requested ────────────────────────────

    #[tokio::test]
    async fn test_resolved_event_received() {
        let broadcaster = ClarifyBroadcaster::new();
        let mut rx = broadcaster.subscribe("conv-2").await;

        let req = make_request("conv-2");
        let clarify_id = req.id.clone();

        broadcaster
            .publish("conv-2", ClarifyEvent::Requested(req))
            .await;
        broadcaster
            .publish(
                "conv-2",
                ClarifyEvent::Resolved {
                    id: clarify_id.clone(),
                    answer: make_answer(),
                },
            )
            .await;

        // First event: Requested
        let e1 = rx.recv().await.unwrap();
        assert!(matches!(e1, ClarifyEvent::Requested(_)));

        // Second event: Resolved
        let e2 = rx.recv().await.unwrap();
        match e2 {
            ClarifyEvent::Resolved { id, .. } => assert_eq!(id, clarify_id),
            _ => panic!("expected Resolved"),
        }
    }

    // ── multiple subscribers each receive the event ────────────────────────

    #[tokio::test]
    async fn test_multiple_subscribers_all_receive() {
        let broadcaster = ClarifyBroadcaster::new();
        let mut rx1 = broadcaster.subscribe("conv-3").await;
        let mut rx2 = broadcaster.subscribe("conv-3").await;

        let req = make_request("conv-3");
        let count = broadcaster
            .publish("conv-3", ClarifyEvent::Requested(req))
            .await;

        assert_eq!(count, 2, "both subscribers should be notified");
        assert!(rx1.recv().await.is_ok());
        assert!(rx2.recv().await.is_ok());
    }

    // ── cleanup_inactive removes channels with no subscribers ─────────────

    #[tokio::test]
    async fn test_cleanup_inactive_removes_empty_channels() {
        let broadcaster = ClarifyBroadcaster::new();

        // Subscribe then drop the receiver.
        {
            let _rx = broadcaster.subscribe("conv-cleanup").await;
            // rx dropped here — receiver_count drops to 0
        }

        broadcaster.cleanup_inactive().await;

        // Channel should be gone; a new publish returns 0 (no channel exists).
        let count = broadcaster
            .publish(
                "conv-cleanup",
                ClarifyEvent::Requested(make_request("conv-cleanup")),
            )
            .await;
        assert_eq!(count, 0);
    }
}
