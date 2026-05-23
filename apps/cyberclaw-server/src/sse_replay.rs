//! Per-conversation SSE event replay buffer.
//!
//! ## Why
//!
//! When a chat SSE stream drops mid-flight (Wi-Fi flicker, proxy timeout,
//! browser tab backgrounded too long), the browser's `EventSource` auto-retries
//! with the last received event's id in the `Last-Event-ID` header. v1.2.14
//! had no buffer for those events, so the reconnect lost everything the
//! server emitted between the original send and the reconnect — leaving the
//! UI showing a half-finished message.
//!
//! ## What it buffers (and what it does NOT)
//!
//! - **Buffers**: every SSE event payload emitted on a chat stream, keyed by
//!   `conversation_id`, with a monotonic per-conversation id starting at 1.
//! - **Does NOT continue inference past disconnect.** When the client drops,
//!   the HTTP request future is dropped, the agent loop stops, no further
//!   events are buffered. Reconnect can only replay what was already
//!   produced. True "background continuation" is a bigger refactor and is
//!   on the v1.3 roadmap.
//!
//! ## Sizing
//!
//! - 64 events per conversation (typical chat turn ≈ 20–40 SSE events)
//! - 1000 conversations max (~ 64k events, ~10 MB worst case)
//! - 5-minute TTL — a dropped client that doesn't reconnect quickly loses
//!   its buffer, freeing space.
//!
//! ## Concurrency
//!
//! Single `RwLock` over the conversation map; per-conversation buffers are
//! mutated under the write lock. For chat-SSE traffic (low write rate,
//! one writer per conversation) this is fine; if we ever need higher
//! contention, switch to `DashMap` or sharded locks.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Maximum events buffered per conversation. Older events are dropped FIFO.
const MAX_EVENTS_PER_CONV: usize = 64;

/// Maximum number of conversations tracked at once. When exceeded, the LRU
/// conversation (by `last_touched`) is evicted before inserting a new one.
const MAX_CONVERSATIONS: usize = 1000;

/// How long a conversation buffer is kept after its last activity.
const DEFAULT_TTL: Duration = Duration::from_secs(300);

/// One conversation's event ring + bookkeeping.
struct ConvBuffer {
    next_id: u64,
    events: VecDeque<(u64, String)>,
    last_touched: Instant,
}

impl ConvBuffer {
    fn new() -> Self {
        Self {
            next_id: 1,
            events: VecDeque::with_capacity(MAX_EVENTS_PER_CONV),
            last_touched: Instant::now(),
        }
    }

    fn append(&mut self, data: String) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        if self.events.len() >= MAX_EVENTS_PER_CONV {
            self.events.pop_front();
        }
        self.events.push_back((id, data));
        self.last_touched = Instant::now();
        id
    }

    fn replay_after(&self, after_id: u64) -> Vec<(u64, String)> {
        self.events
            .iter()
            .filter(|(id, _)| *id > after_id)
            .cloned()
            .collect()
    }
}

/// Shared SSE replay store. Cheap to clone (just an Arc).
#[derive(Clone)]
pub struct SseReplayBuffer {
    inner: Arc<RwLock<HashMap<String, ConvBuffer>>>,
    ttl: Duration,
}

impl SseReplayBuffer {
    /// Construct an empty buffer with the default 5-minute TTL.
    pub fn new() -> Self {
        Self::with_ttl(DEFAULT_TTL)
    }

    /// Construct with a custom TTL (useful for tests).
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            ttl,
        }
    }

    /// Append `data` to the conversation's buffer and return the assigned
    /// monotonic event id. Performs LRU eviction if `MAX_CONVERSATIONS` is
    /// hit and TTL eviction opportunistically.
    pub fn append(&self, conversation_id: &str, data: String) -> u64 {
        let mut map = match self.inner.write() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        self.evict_expired(&mut map);
        if !map.contains_key(conversation_id) && map.len() >= MAX_CONVERSATIONS {
            // Evict LRU.
            if let Some(oldest_key) = map
                .iter()
                .min_by_key(|(_, b)| b.last_touched)
                .map(|(k, _)| k.clone())
            {
                map.remove(&oldest_key);
            }
        }
        let buf = map
            .entry(conversation_id.to_string())
            .or_insert_with(ConvBuffer::new);
        buf.append(data)
    }

    /// Return all buffered events for `conversation_id` whose id is strictly
    /// greater than `after_id`. Empty vec if the conversation isn't buffered.
    pub fn replay_after(&self, conversation_id: &str, after_id: u64) -> Vec<(u64, String)> {
        let map = match self.inner.read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        map.get(conversation_id)
            .map(|b| b.replay_after(after_id))
            .unwrap_or_default()
    }

    /// Drop the buffer for `conversation_id` once the server knows no more
    /// reconnects are expected (e.g. final [DONE] acknowledged client-side).
    /// Idempotent.
    pub fn drop_conversation(&self, conversation_id: &str) {
        let mut map = match self.inner.write() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        map.remove(conversation_id);
    }

    /// Current number of conversations buffered. For metrics/tests.
    pub fn conversation_count(&self) -> usize {
        let map = match self.inner.read() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        map.len()
    }

    fn evict_expired(&self, map: &mut HashMap<String, ConvBuffer>) {
        let cutoff = Instant::now().checked_sub(self.ttl);
        if let Some(cutoff) = cutoff {
            map.retain(|_, b| b.last_touched >= cutoff);
        }
    }
}

impl Default for SseReplayBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse the `Last-Event-ID` header value to a u64. Returns `None` if the
/// header is missing, empty, or not a valid number.
pub fn parse_last_event_id(value: Option<&str>) -> Option<u64> {
    value.and_then(|s| s.trim().parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_assigns_monotonic_ids_starting_at_1() {
        let buf = SseReplayBuffer::new();
        assert_eq!(buf.append("c1", "a".to_string()), 1);
        assert_eq!(buf.append("c1", "b".to_string()), 2);
        assert_eq!(buf.append("c1", "c".to_string()), 3);
    }

    #[test]
    fn ids_are_per_conversation_independent() {
        let buf = SseReplayBuffer::new();
        assert_eq!(buf.append("c1", "x".to_string()), 1);
        assert_eq!(buf.append("c2", "y".to_string()), 1);
        assert_eq!(buf.append("c1", "z".to_string()), 2);
    }

    #[test]
    fn replay_after_returns_only_newer_events() {
        let buf = SseReplayBuffer::new();
        for c in ["a", "b", "c", "d", "e"] {
            buf.append("conv", c.to_string());
        }
        let replayed = buf.replay_after("conv", 2);
        assert_eq!(
            replayed,
            vec![
                (3, "c".to_string()),
                (4, "d".to_string()),
                (5, "e".to_string()),
            ]
        );
    }

    #[test]
    fn replay_after_unknown_conversation_returns_empty() {
        let buf = SseReplayBuffer::new();
        let replayed = buf.replay_after("missing", 0);
        assert!(replayed.is_empty());
    }

    #[test]
    fn append_evicts_oldest_when_capacity_exceeded() {
        let buf = SseReplayBuffer::new();
        for i in 0..(MAX_EVENTS_PER_CONV as u64 + 5) {
            buf.append("c", format!("e{i}"));
        }
        // First 5 events should be evicted.
        let all = buf.replay_after("c", 0);
        assert_eq!(all.len(), MAX_EVENTS_PER_CONV);
        // The oldest still in the buffer should be id == 6 (1..=5 evicted).
        assert_eq!(all.first().unwrap().0, 6);
    }

    #[test]
    fn drop_conversation_removes_buffer() {
        let buf = SseReplayBuffer::new();
        buf.append("c", "x".to_string());
        assert_eq!(buf.conversation_count(), 1);
        buf.drop_conversation("c");
        assert_eq!(buf.conversation_count(), 0);
    }

    #[test]
    fn ttl_expires_idle_conversations_on_next_append() {
        let buf = SseReplayBuffer::with_ttl(Duration::from_millis(10));
        buf.append("c1", "alive".to_string());
        std::thread::sleep(Duration::from_millis(30));
        // Append to a different conv triggers eviction of expired c1.
        buf.append("c2", "fresh".to_string());
        assert_eq!(buf.conversation_count(), 1);
        assert!(buf.replay_after("c1", 0).is_empty());
        assert_eq!(buf.replay_after("c2", 0).len(), 1);
    }

    #[test]
    fn parse_last_event_id_handles_missing_and_garbage() {
        assert_eq!(parse_last_event_id(None), None);
        assert_eq!(parse_last_event_id(Some("")), None);
        assert_eq!(parse_last_event_id(Some("not a number")), None);
        assert_eq!(parse_last_event_id(Some("42")), Some(42));
        assert_eq!(parse_last_event_id(Some(" 7 ")), Some(7));
    }
}
