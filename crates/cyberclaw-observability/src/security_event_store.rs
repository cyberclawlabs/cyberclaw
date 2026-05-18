//! Security event persistent storage for audit and compliance
//!
//! Provides:
//! - [`SecurityEventStore`] trait for pluggable storage backends
//! - [`InMemorySecurityEventStore`] for development and testing
//! - [`EventFilter`] for flexible query filtering

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cyberclaw_core::identity::ActorRef;
use cyberclaw_core::ids::ExecutionId;
use cyberclaw_core::security::{SecurityEvent, SecurityEventType};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Errors that can occur during store operations
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Underlying I/O or serialization error
    #[error("store I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Lock acquisition failed (e.g., poisoned mutex)
    #[error("lock error: {0}")]
    Lock(String),

    /// Generic internal error
    #[error("internal error: {0}")]
    Internal(String),
}

/// Filter for querying security events
///
/// All fields are optional; only non-`None` fields are applied.
/// Multiple filters are combined with logical AND.
#[derive(Debug, Default, Clone)]
pub struct EventFilter {
    /// Only return events for this execution
    pub execution_id: Option<ExecutionId>,

    /// Only return events triggered by this actor (matched by actor id)
    pub actor: Option<ActorRef>,

    /// Only return events of this type
    pub event_type: Option<SecurityEventType>,

    /// Only return events whose timestamp falls within `[start, end]`
    pub time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
}

/// Pluggable storage backend for security events
#[async_trait]
pub trait SecurityEventStore: Send + Sync {
    /// Persist a single security event.
    async fn store(&self, event: SecurityEvent) -> Result<(), StoreError>;

    /// Retrieve events that match all non-`None` filter fields.
    async fn query(&self, filter: EventFilter) -> Result<Vec<SecurityEvent>, StoreError>;

    /// Count events that match all non-`None` filter fields.
    async fn count(&self, filter: EventFilter) -> Result<usize, StoreError>;
}

// ---------------------------------------------------------------------------
// InMemorySecurityEventStore
// ---------------------------------------------------------------------------

/// Thread-safe in-memory implementation of [`SecurityEventStore`].
///
/// Stores events in a `Vec` behind a `RwLock`.  Suitable for unit tests,
/// integration tests, and development; not intended for production persistence.
///
/// # Capacity
/// When `max_events` is `Some(n)`, FIFO eviction is applied so the store never
/// holds more than `n` events.  `None` means unbounded.
#[derive(Clone)]
pub struct InMemorySecurityEventStore {
    inner: Arc<RwLock<Vec<SecurityEvent>>>,
    max_events: Option<usize>,
}

impl Default for InMemorySecurityEventStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemorySecurityEventStore {
    /// Default maximum event count (unbounded)
    pub const DEFAULT_MAX_EVENTS: usize = 100_000;

    /// Create a new unbounded store.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Vec::new())),
            max_events: None,
        }
    }

    /// Create a new store with a maximum capacity.
    ///
    /// Oldest events are evicted (FIFO) when the limit is reached.
    pub fn with_capacity(max_events: usize) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Vec::new())),
            max_events: Some(max_events),
        }
    }
}

#[async_trait]
impl SecurityEventStore for InMemorySecurityEventStore {
    async fn store(&self, event: SecurityEvent) -> Result<(), StoreError> {
        let mut events = self.inner.write().await;

        if let Some(max) = self.max_events {
            if events.len() >= max {
                events.remove(0); // FIFO eviction
            }
        }

        events.push(event);
        drop(events);
        Ok(())
    }

    async fn query(&self, filter: EventFilter) -> Result<Vec<SecurityEvent>, StoreError> {
        let events = self.inner.read().await;
        let result = events
            .iter()
            .filter(|ev| matches_filter(ev, &filter))
            .cloned()
            .collect();
        Ok(result)
    }

    async fn count(&self, filter: EventFilter) -> Result<usize, StoreError> {
        let events = self.inner.read().await;
        let n = events
            .iter()
            .filter(|ev| matches_filter(ev, &filter))
            .count();
        Ok(n)
    }
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

/// Returns `true` when `event` satisfies every non-`None` field in `filter`.
fn matches_filter(event: &SecurityEvent, filter: &EventFilter) -> bool {
    // execution_id filter
    if let Some(ref eid) = filter.execution_id {
        match &event.execution_id {
            Some(ev_eid) if ev_eid == eid => {}
            _ => return false,
        }
    }

    // actor filter – exact match on actor id
    if let Some(ref actor_filter) = filter.actor {
        if event.actor.as_ref().map(|a| &a.id) != Some(&actor_filter.id) {
            return false;
        }
    }

    // event_type filter – compare discriminant via Display-equivalent matching
    if let Some(ref et) = filter.event_type {
        if !event_type_matches(&event.event_type, et) {
            return false;
        }
    }

    // time_range filter – based on event.timestamp
    if let Some((start, end)) = filter.time_range {
        if event.timestamp < start || event.timestamp > end {
            return false;
        }
    }

    true
}

/// Compare two [`SecurityEventType`] values for equality by variant.
///
/// `Custom(a)` vs `Custom(b)` are equal only when `a == b`.
fn event_type_matches(a: &SecurityEventType, b: &SecurityEventType) -> bool {
    matches!(
        (a, b),
        (
            SecurityEventType::PromptInjectionDetected,
            SecurityEventType::PromptInjectionDetected
        ) | (
            SecurityEventType::SkillPoisoningSuspected,
            SecurityEventType::SkillPoisoningSuspected
        ) | (
            SecurityEventType::RuntimeAnomalyDetected,
            SecurityEventType::RuntimeAnomalyDetected
        ) | (
            SecurityEventType::PermissionViolation,
            SecurityEventType::PermissionViolation
        ) | (
            SecurityEventType::PolicyDenied,
            SecurityEventType::PolicyDenied
        )
    ) || matches!((a, b), (SecurityEventType::Custom(x), SecurityEventType::Custom(y)) if x == y)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::identity::ActorType;
    use cyberclaw_core::ids::{ActorId, SecurityEventId, TraceId};
    use cyberclaw_core::security::{SecurityEventSource, Severity};

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_event(
        event_type: SecurityEventType,
        execution_id: Option<ExecutionId>,
    ) -> SecurityEvent {
        // SECURITY FIX: Test helper must use valid ActorRef for audit trail testing
        let test_actor = ActorRef {
            id: ActorId::from_string("test-system".to_string()).unwrap_or_else(|_| ActorId::new()),
            actor_type: ActorType::System,
            tenant_id: None,
            home_node_id: None,
            display_name: "Test System".to_string(),
        };

        SecurityEvent {
            id: SecurityEventId::new(),
            actor: Some(test_actor),
            timestamp: chrono::Utc::now(),
            execution_id,
            case_id: None,
            node_id: None,
            runtime_instance_id: None,
            source: SecurityEventSource::RuntimeDetection,
            event_type,
            severity: Severity::Medium,
            summary: "test event".to_string(),
            details: serde_json::json!({}),
            trace_id: TraceId::new(),
            credential_evidence: None,
        }
    }

    // -----------------------------------------------------------------------
    // Basic store/query tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_store_and_query_all() {
        let store = InMemorySecurityEventStore::new();

        store
            .store(make_event(SecurityEventType::PermissionViolation, None))
            .await
            .unwrap();
        store
            .store(make_event(SecurityEventType::PolicyDenied, None))
            .await
            .unwrap();

        let result = store.query(EventFilter::default()).await.unwrap();
        assert_eq!(result.len(), 2);
    }

    #[tokio::test]
    async fn test_count_all() {
        let store = InMemorySecurityEventStore::new();

        for _ in 0..5 {
            store
                .store(make_event(SecurityEventType::PermissionViolation, None))
                .await
                .unwrap();
        }

        let n = store.count(EventFilter::default()).await.unwrap();
        assert_eq!(n, 5);
    }

    // -----------------------------------------------------------------------
    // Filter: execution_id
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_filter_by_execution_id() {
        let store = InMemorySecurityEventStore::new();

        let exec1 = ExecutionId::from_string("exec-filter-001".to_string()).unwrap();
        let exec2 = ExecutionId::from_string("exec-filter-002".to_string()).unwrap();

        store
            .store(make_event(
                SecurityEventType::PermissionViolation,
                Some(exec1.clone()),
            ))
            .await
            .unwrap();
        store
            .store(make_event(
                SecurityEventType::PolicyDenied,
                Some(exec2.clone()),
            ))
            .await
            .unwrap();
        store
            .store(make_event(
                SecurityEventType::RuntimeAnomalyDetected,
                Some(exec1.clone()),
            ))
            .await
            .unwrap();

        let filter = EventFilter {
            execution_id: Some(exec1.clone()),
            ..Default::default()
        };
        let result = store.query(filter).await.unwrap();
        assert_eq!(result.len(), 2);
        for ev in &result {
            assert_eq!(ev.execution_id.as_ref().unwrap(), &exec1);
        }
    }

    #[tokio::test]
    async fn test_count_filter_by_execution_id() {
        let store = InMemorySecurityEventStore::new();

        let exec1 = ExecutionId::from_string("exec-count-001".to_string()).unwrap();

        for _ in 0..3 {
            store
                .store(make_event(
                    SecurityEventType::PermissionViolation,
                    Some(exec1.clone()),
                ))
                .await
                .unwrap();
        }
        store
            .store(make_event(SecurityEventType::PolicyDenied, None))
            .await
            .unwrap();

        let n = store
            .count(EventFilter {
                execution_id: Some(exec1),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(n, 3);
    }

    // -----------------------------------------------------------------------
    // Filter: event_type
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_filter_by_event_type() {
        let store = InMemorySecurityEventStore::new();

        store
            .store(make_event(SecurityEventType::PermissionViolation, None))
            .await
            .unwrap();
        store
            .store(make_event(SecurityEventType::PolicyDenied, None))
            .await
            .unwrap();
        store
            .store(make_event(SecurityEventType::PermissionViolation, None))
            .await
            .unwrap();

        let filter = EventFilter {
            event_type: Some(SecurityEventType::PermissionViolation),
            ..Default::default()
        };
        let result = store.query(filter).await.unwrap();
        assert_eq!(result.len(), 2);
        for ev in &result {
            assert!(matches!(
                ev.event_type,
                SecurityEventType::PermissionViolation
            ));
        }
    }

    #[tokio::test]
    async fn test_filter_custom_event_type() {
        let store = InMemorySecurityEventStore::new();

        store
            .store(make_event(
                SecurityEventType::Custom("anomaly-x".to_string()),
                None,
            ))
            .await
            .unwrap();
        store
            .store(make_event(
                SecurityEventType::Custom("anomaly-y".to_string()),
                None,
            ))
            .await
            .unwrap();

        let filter = EventFilter {
            event_type: Some(SecurityEventType::Custom("anomaly-x".to_string())),
            ..Default::default()
        };
        let result = store.query(filter).await.unwrap();
        assert_eq!(result.len(), 1);
        assert!(matches!(
            &result[0].event_type,
            SecurityEventType::Custom(s) if s == "anomaly-x"
        ));
    }

    // -----------------------------------------------------------------------
    // Filter: actor (exact match on event.actor field)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_filter_by_actor() {
        use cyberclaw_core::identity::{ActorRef, ActorType};
        use cyberclaw_core::ids::{ActorId, SecurityEventId, TraceId};
        use cyberclaw_core::security::{SecurityEventSource, Severity};

        let store = InMemorySecurityEventStore::new();

        let actor_id = ActorId::from_string("actor-abc-123".to_string()).unwrap();
        let actor = ActorRef {
            id: actor_id.clone(),
            actor_type: ActorType::Agent,
            tenant_id: None,
            home_node_id: None,
            display_name: "TestAgent".to_string(),
        };

        // Event with actor set on the struct field
        store
            .store(SecurityEvent {
                id: SecurityEventId::new(),
                actor: Some(actor.clone()),
                timestamp: chrono::Utc::now(),
                execution_id: None,
                case_id: None,
                node_id: None,
                runtime_instance_id: None,
                source: SecurityEventSource::RuntimeDetection,
                event_type: SecurityEventType::PermissionViolation,
                severity: Severity::High,
                summary: "actor event".to_string(),
                details: serde_json::json!({}),
                trace_id: TraceId::new(),
                credential_evidence: None,
            })
            .await
            .unwrap();

        // Event without actor info
        store
            .store(make_event(SecurityEventType::PolicyDenied, None))
            .await
            .unwrap();

        let filter = EventFilter {
            actor: Some(actor),
            ..Default::default()
        };
        let result = store.query(filter).await.unwrap();
        assert_eq!(result.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Capacity / FIFO eviction
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_fifo_eviction_at_capacity() {
        let store = InMemorySecurityEventStore::with_capacity(5);

        // Store 10 events
        for i in 0..10_u32 {
            let exec_id = ExecutionId::from_string(format!("exec-fifo-{:03}", i)).unwrap();
            store
                .store(make_event(
                    SecurityEventType::PermissionViolation,
                    Some(exec_id),
                ))
                .await
                .unwrap();
        }

        let all = store.query(EventFilter::default()).await.unwrap();
        assert_eq!(all.len(), 5, "should hold exactly capacity events");

        // The 5 newest events (5-9) should remain
        for i in 5..10_u32 {
            let exec_id = ExecutionId::from_string(format!("exec-fifo-{:03}", i)).unwrap();
            let found = all
                .iter()
                .any(|ev| ev.execution_id.as_ref() == Some(&exec_id));
            assert!(found, "exec-fifo-{:03} should be retained", i);
        }
    }

    // -----------------------------------------------------------------------
    // Performance / stress tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_store_1000_events_performance() {
        let store = InMemorySecurityEventStore::new();

        let start = std::time::Instant::now();
        for i in 0..1000_u32 {
            let exec_id = ExecutionId::from_string(format!("perf-exec-{:04}", i)).unwrap();
            store
                .store(make_event(
                    SecurityEventType::PermissionViolation,
                    Some(exec_id),
                ))
                .await
                .unwrap();
        }
        let elapsed = start.elapsed();

        let n = store.count(EventFilter::default()).await.unwrap();
        assert_eq!(n, 1000);

        // 1000 sequential stores should complete well under 1 second
        assert!(
            elapsed.as_millis() < 1000,
            "1000 stores took {}ms (expected < 1000ms)",
            elapsed.as_millis()
        );
    }

    #[tokio::test]
    async fn test_concurrent_writes() {
        use std::sync::Arc;

        let store = Arc::new(InMemorySecurityEventStore::new());
        let mut handles = Vec::new();

        for i in 0..100_u32 {
            let store_clone = Arc::clone(&store);
            let handle = tokio::spawn(async move {
                let exec_id = ExecutionId::from_string(format!("conc-exec-{:04}", i)).unwrap();
                store_clone
                    .store(make_event(
                        SecurityEventType::RuntimeAnomalyDetected,
                        Some(exec_id),
                    ))
                    .await
                    .unwrap();
            });
            handles.push(handle);
        }

        for h in handles {
            h.await.unwrap();
        }

        let n = store.count(EventFilter::default()).await.unwrap();
        assert_eq!(n, 100, "all 100 concurrent writes should succeed");
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_query_empty_store_returns_empty_vec() {
        let store = InMemorySecurityEventStore::new();
        let result = store.query(EventFilter::default()).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_count_empty_store_returns_zero() {
        let store = InMemorySecurityEventStore::new();
        let n = store.count(EventFilter::default()).await.unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn test_query_no_match_returns_empty_vec() {
        let store = InMemorySecurityEventStore::new();

        store
            .store(make_event(SecurityEventType::PolicyDenied, None))
            .await
            .unwrap();

        let exec_id = ExecutionId::from_string("nonexistent-exec".to_string()).unwrap();
        let filter = EventFilter {
            execution_id: Some(exec_id),
            ..Default::default()
        };
        let result = store.query(filter).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_combined_filters() {
        let store = InMemorySecurityEventStore::new();

        let exec1 = ExecutionId::from_string("combo-exec-001".to_string()).unwrap();

        // exec1 + PermissionViolation
        store
            .store(make_event(
                SecurityEventType::PermissionViolation,
                Some(exec1.clone()),
            ))
            .await
            .unwrap();
        // exec1 + PolicyDenied
        store
            .store(make_event(
                SecurityEventType::PolicyDenied,
                Some(exec1.clone()),
            ))
            .await
            .unwrap();
        // no exec + PermissionViolation
        store
            .store(make_event(SecurityEventType::PermissionViolation, None))
            .await
            .unwrap();

        let filter = EventFilter {
            execution_id: Some(exec1),
            event_type: Some(SecurityEventType::PermissionViolation),
            ..Default::default()
        };
        let result = store.query(filter).await.unwrap();
        assert_eq!(result.len(), 1);
        assert!(matches!(
            result[0].event_type,
            SecurityEventType::PermissionViolation
        ));
    }

    // -----------------------------------------------------------------------
    // Clone / Arc sharing
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_clone_shares_backing_store() {
        let store = InMemorySecurityEventStore::new();
        let clone = store.clone();

        store
            .store(make_event(SecurityEventType::PolicyDenied, None))
            .await
            .unwrap();

        let n = clone.count(EventFilter::default()).await.unwrap();
        assert_eq!(n, 1, "cloned handle should see the same backing data");
    }
}
