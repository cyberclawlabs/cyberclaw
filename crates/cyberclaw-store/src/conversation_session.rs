//! v1.3 WP-1 — Server-side conversation session storage.
//!
//! Holds the authoritative per-conversation message history server-side, so
//! clients don't have to re-send `messages: Vec<ChatMessage>` on every turn.
//! This eliminates the lossy round-trip that previously dropped `tool_calls`
//! / `tool_call_id` metadata between CLI requests (R7-01, R9-01, R9-02).
//!
//! # Concepts
//!
//! - [`ConversationId`] — opaque UUID v4 identifier handed back to clients
//!   in the `X-Conversation-Id` response header.
//! - [`ConversationSession`] — full session state: typed [`Message`] list
//!   (with tool_calls preserved), owner (JWT sub), model, agent_id, captured
//!   system prompt, token counters, timestamps.
//! - [`SessionStore`] — async trait abstracting storage backend.
//! - [`InMemorySessionStore`] — process-local `RwLock<HashMap>` with LRU
//!   eviction on overflow + idle-timeout sweep.
//!
//! # Eviction policy (defaults)
//!
//! - Idle timeout: configurable via [`InMemorySessionStore::evict_idle`].
//! - Max sessions: configurable via [`InMemorySessionStore::with_max_sessions`];
//!   on overflow the least-recently-active session is dropped.
//!
//! The server boot path spawns a 60 s sweep that calls `evict_idle` with
//! the operator-configured idle timeout (default 30 min).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cyberclaw_llm::types::Message;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// ConversationId
// ---------------------------------------------------------------------------

/// Opaque conversation identifier (UUID v4 internally).
///
/// Matches the existing `SessionId` / `ExecutionId` patterns in the workspace.
/// Short slugs would require a uniqueness check + central allocator; UUID v4
/// is collision-free for our scale.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationId(String);

impl ConversationId {
    /// Allocate a fresh UUID v4.
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Parse a string as a UUID; rejects non-UUID strings so client-supplied
    /// IDs cannot be squatted for cache-poisoning.
    pub fn from_string(s: impl Into<String>) -> Result<Self, String> {
        let s = s.into();
        Uuid::parse_str(&s).map_err(|e| format!("invalid conversation id: {}", e))?;
        Ok(Self(s))
    }

    /// Borrow the underlying string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ConversationId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ConversationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// ConversationSession
// ---------------------------------------------------------------------------

/// A server-side conversation session.
///
/// `messages` uses the typed [`Message`] from `cyberclaw_llm::types`, which
/// preserves `tool_calls` / `tool_call_id` / `name` — the exact shape
/// `DefaultAgenticLoop` consumes. There is no lossy conversion between
/// what the LLM emits and what the next turn replays.
#[derive(Debug, Clone)]
pub struct ConversationSession {
    /// Stable conversation id (UUID v4).
    pub id: ConversationId,
    /// Full history including tool_calls / tool_call_id metadata.
    pub messages: Vec<Message>,
    /// Owner principal (from JWT `sub` claim). Used for 403 enforcement.
    pub owner: String,
    /// LLM model name captured at session creation.
    pub model: String,
    /// Optional agent identifier captured at session creation.
    pub agent_id: Option<String>,
    /// System prompt captured at session creation. Subsequent turns ignore
    /// the request's `system_prompt` field.
    pub system_prompt: String,
    /// Aggregate token counter across all turns in this session.
    pub total_tokens: u64,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last-activity timestamp; refreshed by [`SessionStore::update`] and
    /// drives idle-timeout eviction.
    pub last_active: DateTime<Utc>,
}

impl ConversationSession {
    /// Construct a fresh session with `created_at` and `last_active` set to
    /// `Utc::now()`. The system prompt and model are captured once and not
    /// overwritten by subsequent turns.
    pub fn new(
        id: ConversationId,
        owner: impl Into<String>,
        model: impl Into<String>,
        agent_id: Option<String>,
        system_prompt: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id,
            messages: Vec::new(),
            owner: owner.into(),
            model: model.into(),
            agent_id,
            system_prompt: system_prompt.into(),
            total_tokens: 0,
            created_at: now,
            last_active: now,
        }
    }
}

// ---------------------------------------------------------------------------
// SessionSummary
// ---------------------------------------------------------------------------

/// Lightweight projection of [`ConversationSession`] used by
/// [`SessionStore::list_active`] (e.g. for admin dashboards).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Conversation id.
    pub id: ConversationId,
    /// Owner principal.
    pub owner: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last-activity timestamp.
    pub last_active: DateTime<Utc>,
    /// Number of messages currently held.
    pub message_count: usize,
}

// ---------------------------------------------------------------------------
// SessionStore trait
// ---------------------------------------------------------------------------

/// Async session-storage abstraction.
///
/// Implementations must be `Send + Sync` so the handler can share an
/// `Arc<dyn SessionStore>` across tasks.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Fetch a session by id. Returns `None` when the id is unknown or has
    /// been evicted.
    async fn get(&self, id: &ConversationId) -> Option<ConversationSession>;

    /// Insert a brand-new session. Returns `Err` if the id is already
    /// present (use [`SessionStore::update`] for mutation).
    async fn put(&self, session: ConversationSession) -> Result<(), String>;

    /// Apply a mutator closure under the store's lock. The closure observes
    /// the latest state and may mutate fields in place. Refreshes
    /// `last_active` on success. Returns `Err` when the id is unknown.
    ///
    /// The closure takes `&mut ConversationSession` via a higher-ranked
    /// trait bound so callers can pass plain `Box::new(|s| { ... })`
    /// without naming a lifetime.
    async fn update(
        &self,
        id: &ConversationId,
        f: Box<dyn for<'a> FnOnce(&'a mut ConversationSession) + Send>,
    ) -> Result<(), String>;

    /// Remove a session by id; returns `true` when one was actually
    /// removed.
    async fn delete(&self, id: &ConversationId) -> bool;

    /// Snapshot of all currently-resident sessions, projected to
    /// [`SessionSummary`].
    async fn list_active(&self) -> Vec<SessionSummary>;

    /// Evict any sessions whose `last_active` is older than `max_idle`.
    /// Returns the number of evicted sessions.
    async fn evict_idle(&self, max_idle: std::time::Duration) -> usize;
}

// ---------------------------------------------------------------------------
// InMemorySessionStore
// ---------------------------------------------------------------------------

/// Process-local session store backed by a single `RwLock<HashMap>`.
///
/// Suitable for single-node v1.3.0; v1.3.1 will swap in a SQLite-backed
/// implementation behind the same trait.
pub struct InMemorySessionStore {
    inner: Arc<RwLock<HashMap<ConversationId, ConversationSession>>>,
    max_sessions: usize,
}

impl InMemorySessionStore {
    /// Default cap of 1000 sessions.
    pub const DEFAULT_MAX_SESSIONS: usize = 1000;

    /// Construct an empty store with the default 1000-session cap.
    pub fn new() -> Self {
        Self::with_max_sessions(Self::DEFAULT_MAX_SESSIONS)
    }

    /// Construct an empty store with an explicit cap. `max_sessions = 0`
    /// is treated as "no cap" so tests can populate freely.
    pub fn with_max_sessions(max_sessions: usize) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            max_sessions,
        }
    }

    /// LRU eviction when the cap is reached: drop the session with the
    /// oldest `last_active`. Called from `put` under an exclusive lock.
    fn evict_oldest_lru(map: &mut HashMap<ConversationId, ConversationSession>) {
        if let Some((victim_id, _)) = map
            .iter()
            .min_by_key(|(_, s)| s.last_active)
            .map(|(id, s)| (id.clone(), s.last_active))
        {
            map.remove(&victim_id);
        }
    }
}

impl Default for InMemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn get(&self, id: &ConversationId) -> Option<ConversationSession> {
        let map = self.inner.read().await;
        map.get(id).cloned()
    }

    async fn put(&self, session: ConversationSession) -> Result<(), String> {
        let mut map = self.inner.write().await;
        if map.contains_key(&session.id) {
            return Err(format!("session {} already exists", session.id));
        }
        if self.max_sessions > 0 && map.len() >= self.max_sessions {
            // LRU: evict before inserting so we never exceed the cap.
            Self::evict_oldest_lru(&mut map);
        }
        map.insert(session.id.clone(), session);
        Ok(())
    }

    async fn update(
        &self,
        id: &ConversationId,
        f: Box<dyn for<'a> FnOnce(&'a mut ConversationSession) + Send>,
    ) -> Result<(), String> {
        let mut map = self.inner.write().await;
        let session = map
            .get_mut(id)
            .ok_or_else(|| format!("session {} not found", id))?;
        f(session);
        session.last_active = Utc::now();
        Ok(())
    }

    async fn delete(&self, id: &ConversationId) -> bool {
        let mut map = self.inner.write().await;
        map.remove(id).is_some()
    }

    async fn list_active(&self) -> Vec<SessionSummary> {
        let map = self.inner.read().await;
        map.values()
            .map(|s| SessionSummary {
                id: s.id.clone(),
                owner: s.owner.clone(),
                created_at: s.created_at,
                last_active: s.last_active,
                message_count: s.messages.len(),
            })
            .collect()
    }

    async fn evict_idle(&self, max_idle: std::time::Duration) -> usize {
        let cutoff = Utc::now()
            - chrono::Duration::from_std(max_idle)
                .unwrap_or_else(|_| chrono::Duration::seconds(1800));
        let mut map = self.inner.write().await;
        let victims: Vec<ConversationId> = map
            .iter()
            .filter(|(_, s)| s.last_active < cutoff)
            .map(|(id, _)| id.clone())
            .collect();
        let n = victims.len();
        for id in victims {
            map.remove(&id);
        }
        n
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_llm::types::Message;

    fn make_session(owner: &str) -> ConversationSession {
        ConversationSession::new(
            ConversationId::new(),
            owner,
            "gpt-4",
            None,
            "test system prompt",
        )
    }

    #[tokio::test]
    async fn test_put_and_get() {
        let store = InMemorySessionStore::new();
        let session = make_session("alice");
        let id = session.id.clone();
        store.put(session).await.expect("put");
        let fetched = store.get(&id).await.expect("get");
        assert_eq!(fetched.owner, "alice");
        assert_eq!(fetched.model, "gpt-4");
    }

    #[tokio::test]
    async fn test_get_missing() {
        let store = InMemorySessionStore::new();
        let id = ConversationId::new();
        assert!(store.get(&id).await.is_none());
    }

    #[tokio::test]
    async fn test_update_appends_message() {
        let store = InMemorySessionStore::new();
        let session = make_session("bob");
        let id = session.id.clone();
        store.put(session).await.expect("put");
        store
            .update(
                &id,
                Box::new(|s| {
                    s.messages.push(Message::user("hello"));
                    s.total_tokens += 42;
                }),
            )
            .await
            .expect("update");
        let updated = store.get(&id).await.expect("get");
        assert_eq!(updated.messages.len(), 1);
        assert_eq!(updated.total_tokens, 42);
    }

    #[tokio::test]
    async fn test_update_missing_returns_err() {
        let store = InMemorySessionStore::new();
        let id = ConversationId::new();
        let result = store
            .update(&id, Box::new(|_| { /* no-op */ }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_delete() {
        let store = InMemorySessionStore::new();
        let session = make_session("carol");
        let id = session.id.clone();
        store.put(session).await.expect("put");
        assert!(store.delete(&id).await);
        assert!(store.get(&id).await.is_none());
        // second delete is a no-op (returns false).
        assert!(!store.delete(&id).await);
    }

    #[tokio::test]
    async fn test_list_active() {
        let store = InMemorySessionStore::new();
        store.put(make_session("a")).await.expect("put");
        store.put(make_session("b")).await.expect("put");
        store.put(make_session("c")).await.expect("put");
        let summaries = store.list_active().await;
        assert_eq!(summaries.len(), 3);
        let owners: Vec<&str> = summaries.iter().map(|s| s.owner.as_str()).collect();
        assert!(owners.contains(&"a"));
        assert!(owners.contains(&"b"));
        assert!(owners.contains(&"c"));
    }

    #[tokio::test]
    async fn test_evict_idle() {
        let store = InMemorySessionStore::new();
        let stale = make_session("stale");
        let stale_id = stale.id.clone();
        store.put(stale).await.expect("put");

        // Force `last_active` into the past via direct update.
        store
            .update(
                &stale_id,
                Box::new(|s| {
                    s.last_active = Utc::now() - chrono::Duration::seconds(7200);
                }),
            )
            .await
            .expect("update");
        // The update() call also refreshes last_active to Utc::now(), so we
        // mutate the map directly to truly simulate an idle session.
        {
            let mut map = store.inner.write().await;
            if let Some(s) = map.get_mut(&stale_id) {
                s.last_active = Utc::now() - chrono::Duration::seconds(7200);
            }
        }

        let fresh = make_session("fresh");
        let fresh_id = fresh.id.clone();
        store.put(fresh).await.expect("put");

        // Evict anything idle > 1h. Stale should disappear; fresh stays.
        let n = store.evict_idle(std::time::Duration::from_secs(3600)).await;
        assert_eq!(n, 1);
        assert!(store.get(&stale_id).await.is_none());
        assert!(store.get(&fresh_id).await.is_some());
    }

    #[tokio::test]
    async fn test_max_sessions_lru() {
        let store = InMemorySessionStore::with_max_sessions(2);
        let oldest = make_session("oldest");
        let oldest_id = oldest.id.clone();
        store.put(oldest).await.expect("put1");

        // Force oldest into the past so it is the LRU victim.
        {
            let mut map = store.inner.write().await;
            if let Some(s) = map.get_mut(&oldest_id) {
                s.last_active = Utc::now() - chrono::Duration::seconds(60);
            }
        }

        let middle = make_session("middle");
        let middle_id = middle.id.clone();
        store.put(middle).await.expect("put2");

        // Adding a third should trigger LRU eviction of `oldest`.
        let newest = make_session("newest");
        let newest_id = newest.id.clone();
        store.put(newest).await.expect("put3");

        assert!(store.get(&oldest_id).await.is_none(), "oldest should be evicted");
        assert!(store.get(&middle_id).await.is_some());
        assert!(store.get(&newest_id).await.is_some());

        let summaries = store.list_active().await;
        assert_eq!(summaries.len(), 2);
    }

    #[test]
    fn test_conversation_id_validation() {
        // Round-trip a freshly-generated UUID.
        let id = ConversationId::new();
        let parsed = ConversationId::from_string(id.as_str()).expect("round-trip");
        assert_eq!(id, parsed);

        // Reject non-UUID strings.
        let err = ConversationId::from_string("not-a-uuid");
        assert!(err.is_err());
        let err = ConversationId::from_string("");
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn test_owner_isolation() {
        // The store doesn't enforce owner — that's the handler's job — but
        // verify the field is preserved verbatim through a put/get round-trip
        // so the handler can rely on it for 403 enforcement.
        let store = InMemorySessionStore::new();
        let session = make_session("alice@example.com");
        let id = session.id.clone();
        store.put(session).await.expect("put");
        let fetched = store.get(&id).await.expect("get");
        assert_eq!(fetched.owner, "alice@example.com");
    }
}
