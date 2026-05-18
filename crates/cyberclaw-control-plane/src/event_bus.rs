use cyberclaw_core::cluster::ClusterEvent;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::warn;
use uuid::Uuid;

/// Subscriber ID for managing subscriptions
pub type SubscriberId = String;

/// Event filter for selective subscription
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventFilter {
    /// Subscribe to all events
    All,
    /// Subscribe to specific event types (by event_type field)
    EventTypes(Vec<String>),
}

/// Event bus configuration
#[derive(Debug, Clone)]
pub struct EventBusConfig {
    /// Maximum number of events buffered per subscriber
    /// Default: 1000 (prevents unbounded memory growth)
    pub subscriber_buffer_size: usize,
}

impl Default for EventBusConfig {
    fn default() -> Self {
        Self {
            subscriber_buffer_size: 1000,
        }
    }
}

impl EventBusConfig {
    /// Minimum buffer size (prevents issues with zero/small buffers)
    const MIN_BUFFER_SIZE: usize = 10;
    /// Maximum buffer size (prevents excessive memory allocation)
    const MAX_BUFFER_SIZE: usize = 100000;

    /// Validate configuration values
    ///
    /// # Security
    /// Validates buffer size to prevent:
    /// - DoS via excessive memory allocation
    /// - Operational issues via too-small buffers
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.subscriber_buffer_size < Self::MIN_BUFFER_SIZE {
            anyhow::bail!(
                "subscriber_buffer_size too low: {} (min {})",
                self.subscriber_buffer_size,
                Self::MIN_BUFFER_SIZE
            );
        }
        if self.subscriber_buffer_size > Self::MAX_BUFFER_SIZE {
            anyhow::bail!(
                "subscriber_buffer_size too high: {} (max {})",
                self.subscriber_buffer_size,
                Self::MAX_BUFFER_SIZE
            );
        }
        Ok(())
    }
}

/// Event subscriber handle
pub struct Subscriber {
    pub id: SubscriberId,
    pub receiver: mpsc::Receiver<ClusterEvent>,
}

/// Event bus trait for publish/subscribe pattern (Milestone C)
pub trait EventBus: Send + Sync {
    /// Subscribe to cluster events with optional filter
    ///
    /// Returns a Subscriber with a receiver channel
    fn subscribe(&self, filter: EventFilter) -> anyhow::Result<Subscriber>;

    /// Unsubscribe from cluster events
    fn unsubscribe(&self, subscriber_id: &SubscriberId) -> anyhow::Result<()>;

    /// Publish a cluster event to all matching subscribers
    fn publish(&self, event: ClusterEvent) -> anyhow::Result<()>;

    /// Get count of active subscribers (for monitoring)
    fn subscriber_count(&self) -> anyhow::Result<usize>;
}

/// Subscriber registration
struct SubscriberRegistration {
    filter: EventFilter,
    sender: mpsc::Sender<ClusterEvent>,
}

/// In-memory implementation of EventBus
///
/// # Concurrency Safety
///
/// The subscriber map is protected by a single `RwLock`.
///
/// - `subscribe` / `unsubscribe`: acquire the **write** lock.
/// - `publish`: acquires the **read** lock to iterate, then upgrades to a
///   **write** lock only when dead subscribers need to be pruned (after
///   releasing the read lock first, avoiding a lock-upgrade deadlock).
///   The cleanup is **best-effort**: a subscriber that is removed concurrently
///   by `unsubscribe` between the read and write phases will simply produce a
///   no-op `remove` call.
/// - `subscriber_count`: acquires the **read** lock.
///
/// Channels use bounded `mpsc` (configured via `EventBusConfig`) so that a
/// slow or malicious subscriber cannot exhaust memory.  Full channels cause
/// event drops for that subscriber only; other subscribers are unaffected.
#[derive(Clone)]
pub struct InMemoryEventBus {
    subscribers: Arc<RwLock<HashMap<SubscriberId, SubscriberRegistration>>>,
    config: EventBusConfig,
}

impl InMemoryEventBus {
    pub fn new(config: EventBusConfig) -> Self {
        Self {
            subscribers: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Check if event matches filter
    fn event_matches_filter(event: &ClusterEvent, filter: &EventFilter) -> bool {
        match filter {
            EventFilter::All => true,
            EventFilter::EventTypes(types) => {
                let event_type = Self::get_event_type(event);
                types.contains(&event_type)
            }
        }
    }

    /// Extract event type from ClusterEvent
    fn get_event_type(event: &ClusterEvent) -> String {
        match event {
            ClusterEvent::ExecutionAssigned { .. } => "execution_assigned".to_string(),
            ClusterEvent::ExecutionLeaseExpired { .. } => "execution_lease_expired".to_string(),
            ClusterEvent::ExecutionReassigned { .. } => "execution_reassigned".to_string(),
            ClusterEvent::WorkerHeartbeatMissed { .. } => "worker_heartbeat_missed".to_string(),
            ClusterEvent::NodeMembershipChanged { .. } => "node_membership_changed".to_string(),
            ClusterEvent::ReviewCreated { .. } => "review_created".to_string(),
            ClusterEvent::ReviewApproved { .. } => "review_approved".to_string(),
            ClusterEvent::ReviewRejected { .. } => "review_rejected".to_string(),
        }
    }
}

impl Default for InMemoryEventBus {
    fn default() -> Self {
        Self::new(EventBusConfig::default())
    }
}

impl EventBus for InMemoryEventBus {
    fn subscribe(&self, filter: EventFilter) -> anyhow::Result<Subscriber> {
        let mut subscribers = self
            .subscribers
            .try_write()
            .map_err(|_| anyhow::anyhow!("failed to acquire write lock on subscribers"))?;

        let subscriber_id = Uuid::new_v4().to_string();

        // Use bounded channel to prevent DoS via unbounded memory growth
        let (sender, receiver) = mpsc::channel(self.config.subscriber_buffer_size);

        let registration = SubscriberRegistration { filter, sender };

        subscribers.insert(subscriber_id.clone(), registration);

        Ok(Subscriber {
            id: subscriber_id,
            receiver,
        })
    }

    fn unsubscribe(&self, subscriber_id: &SubscriberId) -> anyhow::Result<()> {
        let mut subscribers = self
            .subscribers
            .try_write()
            .map_err(|_| anyhow::anyhow!("failed to acquire write lock on subscribers"))?;

        subscribers.remove(subscriber_id);
        Ok(())
    }

    fn publish(&self, event: ClusterEvent) -> anyhow::Result<()> {
        let subscribers = self
            .subscribers
            .try_read()
            .map_err(|_| anyhow::anyhow!("failed to acquire read lock on subscribers"))?;

        // Send to all matching subscribers
        let mut failed_subscribers = Vec::new();
        let mut dropped_events_count = 0;

        for (id, registration) in subscribers.iter() {
            if Self::event_matches_filter(&event, &registration.filter) {
                // Use try_send to avoid blocking if channel is full
                // This prevents a slow consumer from blocking the entire event bus
                match registration.sender.try_send(event.clone()) {
                    Ok(_) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        // Channel full: subscriber is too slow, drop event for this subscriber
                        // In production, this should be logged/monitored
                        dropped_events_count += 1;
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        // Receiver dropped, mark for cleanup
                        failed_subscribers.push(id.clone());
                    }
                }
            }
        }

        // Clean up failed subscribers (receivers dropped)
        if !failed_subscribers.is_empty() {
            drop(subscribers); // Release read lock
            let mut subscribers = self
                .subscribers
                .try_write()
                .map_err(|_| anyhow::anyhow!("failed to acquire write lock for cleanup"))?;

            for id in failed_subscribers {
                subscribers.remove(&id);
            }
        }

        if dropped_events_count > 0 {
            warn!(
                dropped = dropped_events_count,
                "EventBus: events dropped due to full subscriber channels"
            );
        }

        Ok(())
    }

    fn subscriber_count(&self) -> anyhow::Result<usize> {
        let subscribers = self
            .subscribers
            .try_read()
            .map_err(|_| anyhow::anyhow!("failed to acquire read lock on subscribers"))?;

        Ok(subscribers.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use cyberclaw_core::ids::{ExecutionId, LeaseId, NodeId};

    #[tokio::test]
    async fn test_subscribe_all() {
        let bus = InMemoryEventBus::default();

        let mut sub = bus.subscribe(EventFilter::All).unwrap();
        assert_eq!(bus.subscriber_count().unwrap(), 1);

        // Publish event
        let event = ClusterEvent::ExecutionAssigned {
            execution_id: ExecutionId::new(),
            node_id: NodeId::from_string("node-1".to_string()).unwrap(),
            lease_id: LeaseId::new(),
            timestamp: Utc::now(),
        };

        bus.publish(event.clone()).unwrap();

        // Receive event
        let received = sub.receiver.recv().await.unwrap();
        match received {
            ClusterEvent::ExecutionAssigned { .. } => {}
            _ => panic!("unexpected event type"),
        }
    }

    #[tokio::test]
    async fn test_subscribe_filtered() {
        let bus = InMemoryEventBus::default();

        let mut sub = bus
            .subscribe(EventFilter::EventTypes(vec![
                "execution_assigned".to_string()
            ]))
            .unwrap();

        // Publish matching event
        let event1 = ClusterEvent::ExecutionAssigned {
            execution_id: ExecutionId::new(),
            node_id: NodeId::from_string("node-1".to_string()).unwrap(),
            lease_id: LeaseId::new(),
            timestamp: Utc::now(),
        };
        bus.publish(event1).unwrap();

        // Publish non-matching event
        let event2 = ClusterEvent::WorkerHeartbeatMissed {
            node_id: NodeId::from_string("node-1".to_string()).unwrap(),
            last_heartbeat_at: Utc::now(),
            timestamp: Utc::now(),
        };
        bus.publish(event2).unwrap();

        // Should only receive matching event
        let received = sub.receiver.recv().await.unwrap();
        match received {
            ClusterEvent::ExecutionAssigned { .. } => {}
            _ => panic!("unexpected event type"),
        }

        // No more events should be available
        assert!(sub.receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = InMemoryEventBus::default();

        let mut sub1 = bus.subscribe(EventFilter::All).unwrap();
        let mut sub2 = bus.subscribe(EventFilter::All).unwrap();

        assert_eq!(bus.subscriber_count().unwrap(), 2);

        let event = ClusterEvent::ExecutionAssigned {
            execution_id: ExecutionId::new(),
            node_id: NodeId::from_string("node-1".to_string()).unwrap(),
            lease_id: LeaseId::new(),
            timestamp: Utc::now(),
        };

        bus.publish(event).unwrap();

        // Both subscribers should receive event
        assert!(sub1.receiver.recv().await.is_some());
        assert!(sub2.receiver.recv().await.is_some());
    }

    #[tokio::test]
    async fn test_unsubscribe() {
        let bus = InMemoryEventBus::default();

        let sub = bus.subscribe(EventFilter::All).unwrap();
        assert_eq!(bus.subscriber_count().unwrap(), 1);

        bus.unsubscribe(&sub.id).unwrap();
        assert_eq!(bus.subscriber_count().unwrap(), 0);
    }

    #[tokio::test]
    async fn test_auto_cleanup_dropped_receiver() {
        let bus = InMemoryEventBus::default();

        let sub = bus.subscribe(EventFilter::All).unwrap();
        assert_eq!(bus.subscriber_count().unwrap(), 1);

        // Drop receiver
        drop(sub);

        // Publish event (should trigger cleanup)
        let event = ClusterEvent::ExecutionAssigned {
            execution_id: ExecutionId::new(),
            node_id: NodeId::from_string("node-1".to_string()).unwrap(),
            lease_id: LeaseId::new(),
            timestamp: Utc::now(),
        };
        bus.publish(event).unwrap();

        // Subscriber should be cleaned up
        assert_eq!(bus.subscriber_count().unwrap(), 0);
    }

    #[tokio::test]
    async fn test_event_type_filtering() {
        let bus = InMemoryEventBus::default();

        let mut sub = bus
            .subscribe(EventFilter::EventTypes(vec![
                "execution_assigned".to_string(),
                "execution_reassigned".to_string(),
            ]))
            .unwrap();

        // Publish various events
        let events = vec![
            ClusterEvent::ExecutionAssigned {
                execution_id: ExecutionId::new(),
                node_id: NodeId::from_string("node-1".to_string()).unwrap(),
                lease_id: LeaseId::new(),
                timestamp: Utc::now(),
            },
            ClusterEvent::WorkerHeartbeatMissed {
                node_id: NodeId::from_string("node-1".to_string()).unwrap(),
                last_heartbeat_at: Utc::now(),
                timestamp: Utc::now(),
            },
            ClusterEvent::ExecutionReassigned {
                execution_id: ExecutionId::new(),
                old_node_id: NodeId::from_string("node-1".to_string()).unwrap(),
                new_node_id: NodeId::from_string("node-2".to_string()).unwrap(),
                new_lease_id: LeaseId::new(),
                handoff_count: 1,
                timestamp: Utc::now(),
            },
        ];

        for event in events {
            bus.publish(event).unwrap();
        }

        // Should receive 2 events (assigned + reassigned)
        let evt1 = sub.receiver.recv().await.unwrap();
        match evt1 {
            ClusterEvent::ExecutionAssigned { .. } => {}
            _ => panic!("expected ExecutionAssigned"),
        }

        let evt2 = sub.receiver.recv().await.unwrap();
        match evt2 {
            ClusterEvent::ExecutionReassigned { .. } => {}
            _ => panic!("expected ExecutionReassigned"),
        }

        // No more events
        assert!(sub.receiver.try_recv().is_err());
    }

    // Security Test: Bounded Channel DoS Prevention

    #[tokio::test]
    async fn test_bounded_channel_prevents_dos() {
        // Create bus with small buffer for testing
        let config = EventBusConfig {
            subscriber_buffer_size: 10,
        };
        let bus = InMemoryEventBus::new(config);

        // Create subscriber that does NOT consume events (simulates slow/malicious subscriber)
        let _sub = bus.subscribe(EventFilter::All).unwrap();

        // Try to publish more events than buffer capacity
        for i in 0..100 {
            let event = ClusterEvent::ExecutionAssigned {
                execution_id: ExecutionId::new(),
                node_id: NodeId::from_string(format!("node-{}", i)).unwrap(),
                lease_id: LeaseId::new(),
                timestamp: Utc::now(),
            };
            // Should not block or panic, just drop events when buffer full
            bus.publish(event).unwrap();
        }

        // Bus should still be operational
        assert_eq!(bus.subscriber_count().unwrap(), 1);
    }

    #[tokio::test]
    async fn test_multiple_subscribers_independent_buffers() {
        let config = EventBusConfig {
            subscriber_buffer_size: 5,
        };
        let bus = InMemoryEventBus::new(config);

        // Create two subscribers
        let mut fast_sub = bus.subscribe(EventFilter::All).unwrap();
        let _slow_sub = bus.subscribe(EventFilter::All).unwrap(); // Never consumes

        // Publish 10 events
        for i in 0..10 {
            let event = ClusterEvent::ExecutionAssigned {
                execution_id: ExecutionId::new(),
                node_id: NodeId::from_string(format!("node-{}", i)).unwrap(),
                lease_id: LeaseId::new(),
                timestamp: Utc::now(),
            };
            bus.publish(event).unwrap();
        }

        // Fast subscriber should be able to receive events
        // (not blocked by slow subscriber)
        let mut received_count = 0;
        while fast_sub.receiver.try_recv().is_ok() {
            received_count += 1;
        }

        // Fast subscriber should have received at least some events
        // (up to buffer limit of 5)
        assert!(
            received_count >= 5,
            "Fast subscriber received {} events",
            received_count
        );
    }

    #[tokio::test]
    async fn test_large_buffer_size() {
        // Verify we can configure larger buffers for high-throughput scenarios
        let config = EventBusConfig {
            subscriber_buffer_size: 10000,
        };
        let bus = InMemoryEventBus::new(config);

        let mut sub = bus.subscribe(EventFilter::All).unwrap();

        // Publish many events rapidly
        for i in 0..1000 {
            let event = ClusterEvent::ExecutionAssigned {
                execution_id: ExecutionId::new(),
                node_id: NodeId::from_string(format!("node-{}", i)).unwrap(),
                lease_id: LeaseId::new(),
                timestamp: Utc::now(),
            };
            bus.publish(event).unwrap();
        }

        // All events should be buffered successfully
        let mut count = 0;
        while sub.receiver.try_recv().is_ok() {
            count += 1;
        }

        assert_eq!(count, 1000, "All events should be buffered");
    }

    // Configuration Validation Tests

    #[test]
    fn test_config_default_is_valid() {
        let config = EventBusConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_buffer_too_low() {
        let config = EventBusConfig {
            subscriber_buffer_size: 9, // Below MIN (10)
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("subscriber_buffer_size too low"));
    }

    #[test]
    fn test_config_buffer_too_high() {
        let config = EventBusConfig {
            subscriber_buffer_size: 100001, // Above MAX (100000)
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("subscriber_buffer_size too high"));
    }

    #[test]
    fn test_config_valid_edge_cases() {
        // Min value
        let config = EventBusConfig {
            subscriber_buffer_size: 10,
        };
        assert!(config.validate().is_ok());

        // Max value
        let config = EventBusConfig {
            subscriber_buffer_size: 100000,
        };
        assert!(config.validate().is_ok());
    }
}
