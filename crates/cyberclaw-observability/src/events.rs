//! Event recording for execution lifecycle, capability invocations, and security events.
//!
//! This module provides:
//! - [`ObservabilityEvent`]: Lifecycle events for executions, capabilities, and reviews
//! - [`SecurityEvent`]: Security-specific events (re-exported from `cyberclaw-core`)
//! - [`EventRecorder`]: Async trait for recording both event categories
//! - [`InMemoryEventRecorder`]: In-process implementation for development and testing
//!
//! # Design Notes
//!
//! The trait is fully async to avoid fire-and-forget `tokio::spawn` hacks that can
//! cause task leaks and make ordering guarantees impossible in tests. Callers that
//! need non-blocking behaviour are free to `tokio::spawn` the recording call
//! themselves.

use async_trait::async_trait;
use cyberclaw_core::execution::ExecutionStatus;
use cyberclaw_core::identity::ActorRef;
use cyberclaw_core::ids::{CapabilityId, ExecutionId, HandoffId, ReviewId, SessionId};
use cyberclaw_core::review::ReviewTarget;
pub use cyberclaw_core::security::SecurityEvent;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

// ─── Error type ────────────────────────────────────────────────────────────────

/// Errors returned by [`EventRecorder`] methods.
#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    /// The recorder is at capacity and could not accept a new event.
    #[error("recorder at capacity ({0} events); oldest events will be evicted")]
    AtCapacity(usize),
    /// The recorder's internal lock is poisoned or unavailable.
    #[error("recorder lock unavailable: {0}")]
    LockUnavailable(String),
    /// Any other internal error.
    #[error("recorder internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

// ─── ObservabilityEvent ────────────────────────────────────────────────────────

/// Lifecycle event types for observability.
///
/// These events trace the full lifecycle of executions, capabilities, and
/// human-in-the-loop review flows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ObservabilityEvent {
    /// Execution created
    ExecutionCreated {
        execution_id: ExecutionId,
        requested_by: ActorRef,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// Execution status changed
    ExecutionStatusChanged {
        execution_id: ExecutionId,
        from_status: ExecutionStatus,
        to_status: ExecutionStatus,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// Capability invoked
    CapabilityInvoked {
        execution_id: ExecutionId,
        capability_id: CapabilityId,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// Capability completed
    CapabilityCompleted {
        execution_id: ExecutionId,
        capability_id: CapabilityId,
        success: bool,
        duration_ms: u64,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// Review enqueued
    ReviewEnqueued {
        review_id: ReviewId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution_id: Option<ExecutionId>,
        risk_level: String,
        /// S22: discriminated target; defaults to Execution for legacy events.
        #[serde(default)]
        target: ReviewTarget,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// Review approved
    ReviewApproved {
        review_id: ReviewId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution_id: Option<ExecutionId>,
        approved_by: ActorRef,
        wait_time_ms: u64,
        /// S22: discriminated target; defaults to Execution for legacy events.
        #[serde(default)]
        target: ReviewTarget,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// Review rejected
    ReviewRejected {
        review_id: ReviewId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution_id: Option<ExecutionId>,
        rejected_by: ActorRef,
        reason: String,
        /// S22: discriminated target; defaults to Execution for legacy events.
        #[serde(default)]
        target: ReviewTarget,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// Agent execution started
    AgentExecutionStarted {
        execution_id: ExecutionId,
        agent_id: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// Agent execution completed
    AgentExecutionCompleted {
        execution_id: ExecutionId,
        agent_id: String,
        status: ExecutionStatus,
        duration_ms: u64,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// Skill invoked
    SkillInvoked {
        execution_id: ExecutionId,
        skill_id: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// Skill completed
    SkillCompleted {
        execution_id: ExecutionId,
        skill_id: String,
        success: bool,
        duration_ms: u64,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// L1 episodic memory written at execution completion (S18 R1)
    MemoryWritten {
        execution_id: ExecutionId,
        memory_id: String,
        session_id: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// L1 episodic memory read at execution start (S18 R2)
    MemoryRead {
        execution_id: ExecutionId,
        count: usize,
        session_id: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// Multi-agent handoff initiated: agent requested control transfer (S21 T16)
    HandoffInitiated {
        handoff_id: HandoffId,
        from_agent_id: String,
        to_agent_id: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// Multi-agent handoff authorized: governance allowed or denied (S21 T16)
    HandoffAuthorized {
        handoff_id: HandoffId,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// Multi-agent handoff accepted: status transitioned Authorized → Accepted (S21 T4)
    HandoffAccepted {
        handoff_id: HandoffId,
        new_session_id: SessionId,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
}

// ─── EventRecorder trait ───────────────────────────────────────────────────────

/// Async trait for recording observability and security events.
///
/// Implementations must be `Send + Sync` so they can be shared across
/// Tokio tasks via `Arc<dyn EventRecorder>`.
///
/// # Backward compatibility
///
/// The original `record` / `get_events` / `clear` surface is preserved as
/// [`record_event`] / [`get_events`] / [`clear`] to avoid breaking callers.
/// New code should prefer [`record_security_event`] for security-specific
/// events.
///
/// # Examples
///
/// ```rust,no_run
/// use cyberclaw_observability::events::{EventRecorder, InMemoryEventRecorder};
/// use cyberclaw_core::security::{SecurityEvent, SecurityEventSource, SecurityEventType, Severity};
/// use cyberclaw_core::ids::{SecurityEventId, TraceId};
///
/// # async fn run() -> anyhow::Result<()> {
/// let recorder = InMemoryEventRecorder::new();
///
/// let event = SecurityEvent {
///     id: SecurityEventId::new(),
///     actor: None,
///     timestamp: chrono::Utc::now(),
///     execution_id: None,
///     case_id: None,
///     node_id: None,
///     runtime_instance_id: None,
///     source: SecurityEventSource::PolicyEngine,
///     event_type: SecurityEventType::PolicyDenied,
///     severity: Severity::High,
///     summary: "Capability denied by policy".to_string(),
///     details: serde_json::json!({}),
///     trace_id: TraceId::new(),
///     credential_evidence: None,
/// };
///
/// recorder.record_security_event(event).await?;
/// # Ok(())
/// # }
/// ```
#[async_trait]
pub trait EventRecorder: Send + Sync {
    /// Record a lifecycle observability event.
    ///
    /// This is the primary method for recording execution, capability, and
    /// review lifecycle events.
    async fn record_event(&self, event: ObservabilityEvent) -> Result<(), RecordError>;

    /// Record a security event using the unified [`SecurityEvent`] model.
    ///
    /// Security events are stored in a separate store from lifecycle events to
    /// allow targeted querying (e.g. "all policy denials in the last hour").
    async fn record_security_event(&self, event: SecurityEvent) -> Result<(), RecordError>;

    /// Return all recorded lifecycle events.
    ///
    /// Primarily for testing and debugging. May block briefly to acquire the
    /// internal read lock.
    async fn get_events(&self) -> Result<Vec<ObservabilityEvent>, RecordError>;

    /// Return all recorded security events.
    ///
    /// Primarily for testing and debugging.
    async fn get_security_events(&self) -> Result<Vec<SecurityEvent>, RecordError>;

    /// Clear all recorded lifecycle events.
    async fn clear(&self) -> Result<(), RecordError>;

    /// Clear all recorded security events.
    async fn clear_security_events(&self) -> Result<(), RecordError>;
}

// ─── InMemoryEventRecorder ────────────────────────────────────────────────────

/// In-memory event recorder for development and testing.
///
/// Both lifecycle events and security events are stored in separate bounded
/// ring-buffers. When a buffer reaches `max_events`, the oldest entry is
/// evicted (FIFO).
///
/// The recorder is fully async: all operations `await` the internal
/// `RwLock`, so there are no fire-and-forget spawns or timing races.
///
/// # Cloning
///
/// Cloning produces a handle to the *same* internal storage (via `Arc`),
/// which is safe to share across tasks.
#[derive(Clone)]
pub struct InMemoryEventRecorder {
    events: Arc<RwLock<Vec<ObservabilityEvent>>>,
    security_events: Arc<RwLock<Vec<SecurityEvent>>>,
    max_events: usize,
}

impl Default for InMemoryEventRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryEventRecorder {
    /// Default maximum number of events to store per channel.
    pub const DEFAULT_MAX_EVENTS: usize = 10_000;

    /// Create a new recorder with default capacity.
    pub fn new() -> Self {
        Self::with_capacity(Self::DEFAULT_MAX_EVENTS)
    }

    /// Create a new recorder with a custom per-channel capacity.
    pub fn with_capacity(max_events: usize) -> Self {
        Self {
            events: Arc::new(RwLock::new(Vec::new())),
            security_events: Arc::new(RwLock::new(Vec::new())),
            max_events,
        }
    }

    // ── Lifecycle-event helpers ────────────────────────────────────────────────

    /// Return all lifecycle events whose `execution_id` matches the given value.
    pub async fn get_events_for_execution(
        &self,
        execution_id: &ExecutionId,
    ) -> Vec<ObservabilityEvent> {
        let events = self.events.read().await;
        events
            .iter()
            .filter(|event| match event {
                ObservabilityEvent::ExecutionCreated {
                    execution_id: eid, ..
                } => eid == execution_id,
                ObservabilityEvent::ExecutionStatusChanged {
                    execution_id: eid, ..
                } => eid == execution_id,
                ObservabilityEvent::CapabilityInvoked {
                    execution_id: eid, ..
                } => eid == execution_id,
                ObservabilityEvent::CapabilityCompleted {
                    execution_id: eid, ..
                } => eid == execution_id,
                ObservabilityEvent::ReviewEnqueued {
                    execution_id: eid, ..
                } => eid.as_ref() == Some(execution_id),
                ObservabilityEvent::ReviewApproved {
                    execution_id: eid, ..
                } => eid.as_ref() == Some(execution_id),
                ObservabilityEvent::ReviewRejected {
                    execution_id: eid, ..
                } => eid.as_ref() == Some(execution_id),
                ObservabilityEvent::AgentExecutionStarted {
                    execution_id: eid, ..
                } => eid == execution_id,
                ObservabilityEvent::AgentExecutionCompleted {
                    execution_id: eid, ..
                } => eid == execution_id,
                ObservabilityEvent::SkillInvoked {
                    execution_id: eid, ..
                } => eid == execution_id,
                ObservabilityEvent::SkillCompleted {
                    execution_id: eid, ..
                } => eid == execution_id,
                ObservabilityEvent::MemoryWritten {
                    execution_id: eid, ..
                } => eid == execution_id,
                ObservabilityEvent::MemoryRead {
                    execution_id: eid, ..
                } => eid == execution_id,
                ObservabilityEvent::HandoffInitiated { .. } => false,
                ObservabilityEvent::HandoffAuthorized { .. } => false,
                ObservabilityEvent::HandoffAccepted { .. } => false,
            })
            .cloned()
            .collect()
    }

    /// Count lifecycle events matching the given variant name (e.g. `"ExecutionCreated"`).
    pub async fn count_events_by_type(&self, event_type: &str) -> usize {
        let events = self.events.read().await;
        events
            .iter()
            .filter(|event| {
                let name = match event {
                    ObservabilityEvent::ExecutionCreated { .. } => "ExecutionCreated",
                    ObservabilityEvent::ExecutionStatusChanged { .. } => "ExecutionStatusChanged",
                    ObservabilityEvent::CapabilityInvoked { .. } => "CapabilityInvoked",
                    ObservabilityEvent::CapabilityCompleted { .. } => "CapabilityCompleted",
                    ObservabilityEvent::ReviewEnqueued { .. } => "ReviewEnqueued",
                    ObservabilityEvent::ReviewApproved { .. } => "ReviewApproved",
                    ObservabilityEvent::ReviewRejected { .. } => "ReviewRejected",
                    ObservabilityEvent::AgentExecutionStarted { .. } => "AgentExecutionStarted",
                    ObservabilityEvent::AgentExecutionCompleted { .. } => "AgentExecutionCompleted",
                    ObservabilityEvent::SkillInvoked { .. } => "SkillInvoked",
                    ObservabilityEvent::SkillCompleted { .. } => "SkillCompleted",
                    ObservabilityEvent::MemoryWritten { .. } => "MemoryWritten",
                    ObservabilityEvent::MemoryRead { .. } => "MemoryRead",
                    ObservabilityEvent::HandoffInitiated { .. } => "HandoffInitiated",
                    ObservabilityEvent::HandoffAuthorized { .. } => "HandoffAuthorized",
                    ObservabilityEvent::HandoffAccepted { .. } => "HandoffAccepted",
                };
                name == event_type
            })
            .count()
    }

    // ── Security-event helpers ─────────────────────────────────────────────────

    /// Return all security events associated with the given execution ID.
    pub async fn get_security_events_for_execution(
        &self,
        execution_id: &ExecutionId,
    ) -> Vec<SecurityEvent> {
        let events = self.security_events.read().await;
        events
            .iter()
            .filter(|e| e.execution_id.as_ref() == Some(execution_id))
            .cloned()
            .collect()
    }

    /// Count security events matching the given severity label (e.g. `"Critical"`).
    pub async fn count_security_events_by_severity(&self, severity: &str) -> usize {
        use cyberclaw_core::security::Severity;
        let events = self.security_events.read().await;
        events
            .iter()
            .filter(|e| {
                let label = match &e.severity {
                    Severity::Info => "Info",
                    Severity::Low => "Low",
                    Severity::Medium => "Medium",
                    Severity::High => "High",
                    Severity::Critical => "Critical",
                };
                label == severity
            })
            .count()
    }
}

// ─── EventRecorder impl ────────────────────────────────────────────────────────

#[async_trait]
impl EventRecorder for InMemoryEventRecorder {
    async fn record_event(&self, event: ObservabilityEvent) -> Result<(), RecordError> {
        let mut events = self.events.write().await;
        if events.len() >= self.max_events {
            events.remove(0); // FIFO eviction
        }
        events.push(event);
        Ok(())
    }

    async fn record_security_event(&self, event: SecurityEvent) -> Result<(), RecordError> {
        let mut events = self.security_events.write().await;
        if events.len() >= self.max_events {
            events.remove(0); // FIFO eviction
        }
        events.push(event);
        Ok(())
    }

    async fn get_events(&self) -> Result<Vec<ObservabilityEvent>, RecordError> {
        let events = self.events.read().await;
        Ok(events.clone())
    }

    async fn get_security_events(&self) -> Result<Vec<SecurityEvent>, RecordError> {
        let events = self.security_events.read().await;
        Ok(events.clone())
    }

    async fn clear(&self) -> Result<(), RecordError> {
        let mut events = self.events.write().await;
        events.clear();
        Ok(())
    }

    async fn clear_security_events(&self) -> Result<(), RecordError> {
        let mut events = self.security_events.write().await;
        events.clear();
        Ok(())
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::execution::ExecutionStatus;
    use cyberclaw_core::identity::{ActorRef, ActorType};
    use cyberclaw_core::ids::{ActorId, ExecutionId, SecurityEventId, TraceId};
    use cyberclaw_core::security::{SecurityEventSource, SecurityEventType, Severity};

    fn make_actor(id: &str) -> ActorRef {
        ActorRef {
            id: ActorId::from_string(id.to_string()).unwrap(),
            actor_type: ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: "Test Actor".to_string(),
        }
    }

    fn make_security_event(execution_id: Option<ExecutionId>) -> SecurityEvent {
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
            source: SecurityEventSource::PolicyEngine,
            event_type: SecurityEventType::PolicyDenied,
            severity: Severity::High,
            summary: "Policy denied capability".to_string(),
            details: serde_json::json!({"capability": "file:write"}),
            trace_id: TraceId::new(),
            credential_evidence: None,
        }
    }

    // ── Lifecycle event tests ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_record_and_get_lifecycle_event() {
        let recorder = InMemoryEventRecorder::new();
        let exec_id = ExecutionId::from_string("exec-001".to_string()).unwrap();

        recorder
            .record_event(ObservabilityEvent::ExecutionCreated {
                execution_id: exec_id.clone(),
                requested_by: make_actor("user-1"),
                timestamp: chrono::Utc::now(),
            })
            .await
            .unwrap();

        let events = recorder.get_events().await.unwrap();
        assert_eq!(events.len(), 1);

        let by_exec = recorder.get_events_for_execution(&exec_id).await;
        assert_eq!(by_exec.len(), 1);
    }

    #[tokio::test]
    async fn test_lifecycle_event_filtering_by_execution_id() {
        let recorder = InMemoryEventRecorder::new();
        let exec1 = ExecutionId::from_string("exec-001".to_string()).unwrap();
        let exec2 = ExecutionId::from_string("exec-002".to_string()).unwrap();

        recorder
            .record_event(ObservabilityEvent::ExecutionCreated {
                execution_id: exec1.clone(),
                requested_by: make_actor("user-1"),
                timestamp: chrono::Utc::now(),
            })
            .await
            .unwrap();

        recorder
            .record_event(ObservabilityEvent::ExecutionCreated {
                execution_id: exec2.clone(),
                requested_by: make_actor("user-2"),
                timestamp: chrono::Utc::now(),
            })
            .await
            .unwrap();

        assert_eq!(recorder.get_events_for_execution(&exec1).await.len(), 1);
        assert_eq!(recorder.get_events_for_execution(&exec2).await.len(), 1);
    }

    #[tokio::test]
    async fn test_lifecycle_event_type_counting() {
        let recorder = InMemoryEventRecorder::new();
        let exec_id = ExecutionId::from_string("exec-001".to_string()).unwrap();

        recorder
            .record_event(ObservabilityEvent::ExecutionCreated {
                execution_id: exec_id.clone(),
                requested_by: make_actor("user-1"),
                timestamp: chrono::Utc::now(),
            })
            .await
            .unwrap();

        recorder
            .record_event(ObservabilityEvent::ExecutionStatusChanged {
                execution_id: exec_id.clone(),
                from_status: ExecutionStatus::Pending,
                to_status: ExecutionStatus::Running,
                timestamp: chrono::Utc::now(),
            })
            .await
            .unwrap();

        assert_eq!(recorder.count_events_by_type("ExecutionCreated").await, 1);
        assert_eq!(
            recorder
                .count_events_by_type("ExecutionStatusChanged")
                .await,
            1
        );
    }

    #[tokio::test]
    async fn test_lifecycle_clear() {
        let recorder = InMemoryEventRecorder::new();
        let exec_id = ExecutionId::from_string("exec-001".to_string()).unwrap();

        recorder
            .record_event(ObservabilityEvent::ExecutionCreated {
                execution_id: exec_id.clone(),
                requested_by: make_actor("user-1"),
                timestamp: chrono::Utc::now(),
            })
            .await
            .unwrap();

        assert_eq!(recorder.get_events().await.unwrap().len(), 1);

        recorder.clear().await.unwrap();

        assert_eq!(recorder.get_events().await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_lifecycle_fifo_eviction() {
        let recorder = InMemoryEventRecorder::with_capacity(3);

        for i in 0..5u32 {
            recorder
                .record_event(ObservabilityEvent::ExecutionCreated {
                    execution_id: ExecutionId::from_string(format!("exec-{:03}", i)).unwrap(),
                    requested_by: make_actor("user-1"),
                    timestamp: chrono::Utc::now(),
                })
                .await
                .unwrap();
        }

        let events = recorder.get_events().await.unwrap();
        assert_eq!(
            events.len(),
            3,
            "should have exactly 3 events after FIFO eviction"
        );

        // The latest 3 events should be exec-002, exec-003, exec-004
        for (idx, event) in events.iter().enumerate() {
            if let ObservabilityEvent::ExecutionCreated { execution_id, .. } = event {
                let expected = ExecutionId::from_string(format!("exec-{:03}", idx + 2)).unwrap();
                assert_eq!(execution_id, &expected);
            } else {
                panic!("unexpected event variant");
            }
        }
    }

    // ── Security event tests ───────────────────────────────────────────────────

    #[tokio::test]
    async fn test_record_security_event() {
        let recorder = InMemoryEventRecorder::new();
        let event = make_security_event(None);

        recorder.record_security_event(event).await.unwrap();

        let events = recorder.get_security_events().await.unwrap();
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn test_security_event_filter_by_execution_id() {
        let recorder = InMemoryEventRecorder::new();
        let exec_id = ExecutionId::from_string("exec-sec-001".to_string()).unwrap();

        // One event tied to exec_id, one without
        recorder
            .record_security_event(make_security_event(Some(exec_id.clone())))
            .await
            .unwrap();
        recorder
            .record_security_event(make_security_event(None))
            .await
            .unwrap();

        let for_exec = recorder.get_security_events_for_execution(&exec_id).await;
        assert_eq!(for_exec.len(), 1);

        let all = recorder.get_security_events().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_security_event_severity_counting() {
        let recorder = InMemoryEventRecorder::new();

        // Record 2 High, 1 Critical
        for _ in 0..2 {
            recorder
                .record_security_event(SecurityEvent {
                    severity: Severity::High,
                    ..make_security_event(None)
                })
                .await
                .unwrap();
        }
        recorder
            .record_security_event(SecurityEvent {
                severity: Severity::Critical,
                ..make_security_event(None)
            })
            .await
            .unwrap();

        assert_eq!(recorder.count_security_events_by_severity("High").await, 2);
        assert_eq!(
            recorder.count_security_events_by_severity("Critical").await,
            1
        );
        assert_eq!(recorder.count_security_events_by_severity("Low").await, 0);
    }

    #[tokio::test]
    async fn test_security_event_clear() {
        let recorder = InMemoryEventRecorder::new();

        recorder
            .record_security_event(make_security_event(None))
            .await
            .unwrap();

        assert_eq!(recorder.get_security_events().await.unwrap().len(), 1);

        recorder.clear_security_events().await.unwrap();

        assert_eq!(recorder.get_security_events().await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_security_event_fifo_eviction() {
        let recorder = InMemoryEventRecorder::with_capacity(3);

        for _ in 0..5 {
            recorder
                .record_security_event(make_security_event(None))
                .await
                .unwrap();
        }

        let events = recorder.get_security_events().await.unwrap();
        assert_eq!(events.len(), 3);
    }

    #[tokio::test]
    async fn test_security_event_policy_denied() {
        let recorder = InMemoryEventRecorder::new();
        let exec_id = ExecutionId::from_string("exec-gov-001".to_string()).unwrap();

        // SECURITY FIX: Create test actor for audit trail
        let test_actor = ActorRef {
            id: ActorId::from_string("user-123".to_string()).unwrap_or_else(|_| ActorId::new()),
            actor_type: ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: "Test User 123".to_string(),
        };

        let event = SecurityEvent {
            id: SecurityEventId::new(),
            actor: Some(test_actor),
            timestamp: chrono::Utc::now(),
            execution_id: Some(exec_id.clone()),
            case_id: None,
            node_id: None,
            runtime_instance_id: None,
            source: SecurityEventSource::PolicyEngine,
            event_type: SecurityEventType::PolicyDenied,
            severity: Severity::High,
            summary: "Capability 'file:delete' denied by governance policy".to_string(),
            details: serde_json::json!({
                "capability": "file:delete",
                "policy": "no-destructive-ops",
                "actor": "user-123"
            }),
            trace_id: TraceId::new(),
            credential_evidence: None,
        };

        recorder.record_security_event(event).await.unwrap();

        let security = recorder.get_security_events_for_execution(&exec_id).await;
        assert_eq!(security.len(), 1);
        assert!(matches!(
            security[0].event_type,
            SecurityEventType::PolicyDenied
        ));
    }

    #[tokio::test]
    async fn test_security_event_permission_violation() {
        let recorder = InMemoryEventRecorder::new();

        // SECURITY FIX: Create test actor for audit trail
        let bad_agent = ActorRef {
            id: ActorId::from_string("bad-agent".to_string()).unwrap_or_else(|_| ActorId::new()),
            actor_type: ActorType::Agent,
            tenant_id: None,
            home_node_id: None,
            display_name: "Bad Agent".to_string(),
        };

        let event = SecurityEvent {
            id: SecurityEventId::new(),
            actor: Some(bad_agent),
            timestamp: chrono::Utc::now(),
            execution_id: None,
            case_id: None,
            node_id: None,
            runtime_instance_id: Some("runtime-abc".to_string()),
            source: SecurityEventSource::PermissionEngine,
            event_type: SecurityEventType::PermissionViolation,
            severity: Severity::Critical,
            summary: "Agent attempted to access resource outside permitted scope".to_string(),
            details: serde_json::json!({"resource": "/etc/shadow", "agent": "bad-agent"}),
            trace_id: TraceId::new(),
            credential_evidence: None,
        };

        recorder.record_security_event(event).await.unwrap();

        let critical_count = recorder.count_security_events_by_severity("Critical").await;
        assert_eq!(critical_count, 1);
    }

    #[tokio::test]
    async fn test_security_event_prompt_injection_detected() {
        let recorder = InMemoryEventRecorder::new();

        // SECURITY FIX: Create test actor for audit trail
        let scanner_system = ActorRef {
            id: ActorId::from_string("prompt-scanner".to_string())
                .unwrap_or_else(|_| ActorId::new()),
            actor_type: ActorType::System,
            tenant_id: None,
            home_node_id: None,
            display_name: "Prompt Scanner".to_string(),
        };

        let event = SecurityEvent {
            id: SecurityEventId::new(),
            actor: Some(scanner_system),
            timestamp: chrono::Utc::now(),
            execution_id: None,
            case_id: None,
            node_id: None,
            runtime_instance_id: None,
            source: SecurityEventSource::PromptScanner,
            event_type: SecurityEventType::PromptInjectionDetected,
            severity: Severity::High,
            summary: "Prompt injection attempt detected in user input".to_string(),
            details: serde_json::json!({"input_hash": "sha256:deadbeef", "confidence": 0.97}),
            trace_id: TraceId::new(),
            credential_evidence: None,
        };

        recorder.record_security_event(event).await.unwrap();

        let events = recorder.get_security_events().await.unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0].event_type,
            SecurityEventType::PromptInjectionDetected
        ));
    }

    // ── S21 T16: HandoffInitiated / HandoffAuthorized variant tests ────────────

    #[tokio::test]
    async fn handoff_initiated_event_serializes() {
        use cyberclaw_core::ids::HandoffId;
        let recorder = InMemoryEventRecorder::new();
        let hid = HandoffId::from_string("ho_test_001".to_string()).unwrap();

        let event = ObservabilityEvent::HandoffInitiated {
            handoff_id: hid.clone(),
            from_agent_id: "agent_a".to_string(),
            to_agent_id: "agent_b".to_string(),
            timestamp: chrono::Utc::now(),
        };

        // Roundtrip through serde_json.
        let json = serde_json::to_string(&event).expect("serialize HandoffInitiated");
        let back: ObservabilityEvent =
            serde_json::from_str(&json).expect("deserialize HandoffInitiated");

        // Verify discriminant and fields are preserved.
        assert!(matches!(back, ObservabilityEvent::HandoffInitiated { .. }));
        if let ObservabilityEvent::HandoffInitiated {
            handoff_id,
            from_agent_id,
            to_agent_id,
            ..
        } = back
        {
            assert_eq!(handoff_id, hid);
            assert_eq!(from_agent_id, "agent_a");
            assert_eq!(to_agent_id, "agent_b");
        }

        // Record + count.
        recorder.record_event(event).await.unwrap();
        assert_eq!(recorder.count_events_by_type("HandoffInitiated").await, 1);
        assert_eq!(recorder.count_events_by_type("HandoffAuthorized").await, 0);
    }

    #[tokio::test]
    async fn handoff_authorized_event_serializes() {
        use cyberclaw_core::ids::HandoffId;
        let recorder = InMemoryEventRecorder::new();
        let hid = HandoffId::from_string("ho_test_002".to_string()).unwrap();

        let event = ObservabilityEvent::HandoffAuthorized {
            handoff_id: hid.clone(),
            timestamp: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&event).expect("serialize HandoffAuthorized");
        let back: ObservabilityEvent =
            serde_json::from_str(&json).expect("deserialize HandoffAuthorized");

        assert!(matches!(back, ObservabilityEvent::HandoffAuthorized { .. }));
        if let ObservabilityEvent::HandoffAuthorized { handoff_id, .. } = back {
            assert_eq!(handoff_id, hid);
        }

        recorder.record_event(event).await.unwrap();
        assert_eq!(recorder.count_events_by_type("HandoffAuthorized").await, 1);
        // HandoffInitiated not recorded — still 0.
        assert_eq!(recorder.count_events_by_type("HandoffInitiated").await, 0);
    }

    #[tokio::test]
    async fn handoff_variants_not_execution_scoped() {
        use cyberclaw_core::ids::{ExecutionId, HandoffId};
        let recorder = InMemoryEventRecorder::new();
        let exec_id = ExecutionId::from_string("exec-handoff-scope".to_string()).unwrap();
        let hid = HandoffId::from_string("ho_scope_001".to_string()).unwrap();

        recorder
            .record_event(ObservabilityEvent::HandoffInitiated {
                handoff_id: hid.clone(),
                from_agent_id: "a".to_string(),
                to_agent_id: "b".to_string(),
                timestamp: chrono::Utc::now(),
            })
            .await
            .unwrap();
        recorder
            .record_event(ObservabilityEvent::HandoffAuthorized {
                handoff_id: hid.clone(),
                timestamp: chrono::Utc::now(),
            })
            .await
            .unwrap();

        // Handoff variants are not execution-scoped — must not appear in result.
        let scoped = recorder.get_events_for_execution(&exec_id).await;
        assert!(
            scoped.is_empty(),
            "handoff events must not be returned by get_events_for_execution"
        );
    }

    #[tokio::test]
    async fn test_lifecycle_and_security_events_are_independent() {
        let recorder = InMemoryEventRecorder::new();
        let exec_id = ExecutionId::from_string("exec-mixed-001".to_string()).unwrap();

        // Record both kinds
        recorder
            .record_event(ObservabilityEvent::ExecutionCreated {
                execution_id: exec_id.clone(),
                requested_by: make_actor("user-1"),
                timestamp: chrono::Utc::now(),
            })
            .await
            .unwrap();

        recorder
            .record_security_event(make_security_event(Some(exec_id.clone())))
            .await
            .unwrap();

        // Clearing lifecycle events should not affect security events
        recorder.clear().await.unwrap();

        assert_eq!(recorder.get_events().await.unwrap().len(), 0);
        assert_eq!(recorder.get_security_events().await.unwrap().len(), 1);
    }

    // ── S22 T5: ReviewEnqueued target serde roundtrip ──────────────────────────

    #[tokio::test]
    async fn review_enqueued_event_with_target_serde_roundtrip() {
        use cyberclaw_core::ids::ReviewId;
        use cyberclaw_core::review::ReviewTarget;

        let exec_id = ExecutionId::from_string("exec_t5".to_string()).unwrap();
        let event = ObservabilityEvent::ReviewEnqueued {
            review_id: ReviewId::from_string("rev_t5".to_string()).unwrap(),
            execution_id: Some(exec_id.clone()),
            risk_level: "High".to_string(),
            target: ReviewTarget::Execution {
                execution_id: exec_id.clone(),
            },
            timestamp: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&event).expect("serialize ReviewEnqueued");
        let back: ObservabilityEvent =
            serde_json::from_str(&json).expect("deserialize ReviewEnqueued");

        if let ObservabilityEvent::ReviewEnqueued { target, .. } = back {
            assert!(target.execution_id().is_some());
        } else {
            panic!("expected ReviewEnqueued");
        }
    }

    #[test]
    fn review_enqueued_legacy_no_target_defaults_to_execution() {
        // JSON without target field must deserialize via default_target_execution.
        // ObservabilityEvent uses externally-tagged serde (no tag attr), so variant
        // name is the outer key.
        let json = r#"{"ReviewEnqueued":{"review_id":"rev_legacy","execution_id":"exec_old","risk_level":"Medium","timestamp":"2026-01-01T00:00:00Z"}}"#;
        let event: ObservabilityEvent =
            serde_json::from_str(json).expect("legacy ReviewEnqueued deserializes");
        if let ObservabilityEvent::ReviewEnqueued { target, .. } = event {
            assert!(matches!(
                target,
                cyberclaw_core::review::ReviewTarget::Execution { .. }
            ));
        } else {
            panic!("expected ReviewEnqueued");
        }
    }
}
