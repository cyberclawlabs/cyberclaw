use cyberclaw_core::prelude::*;
use cyberclaw_core::security::{SecurityEvent, SecurityEventSource, SecurityEventType, Severity};
use cyberclaw_observability::security_event_store::SecurityEventStore;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;

#[async_trait::async_trait]
pub trait ReviewQueue: Send + Sync {
    async fn enqueue(&self, review: ReviewRequest) -> anyhow::Result<()>;
    async fn list_pending(&self) -> anyhow::Result<Vec<ReviewRequest>>;
    async fn get(&self, review_id: &ReviewId) -> Option<ReviewRequest>;
    /// Approve a review request
    ///
    /// # Security
    /// CRITICAL: The approver parameter must be the actual reviewer performing the approval,
    /// not the original requester. This ensures proper audit trail (OWASP A09).
    async fn approve(&self, review_id: &ReviewId, approver: &ActorRef) -> anyhow::Result<()>;
    /// Reject a review request
    ///
    /// # Security
    /// CRITICAL: The approver parameter must be the actual reviewer performing the rejection,
    /// not the original requester. This ensures proper audit trail (OWASP A09).
    async fn reject(&self, review_id: &ReviewId, approver: &ActorRef) -> anyhow::Result<()>;
}

/// InMemory implementation of ReviewQueue for development and testing
#[derive(Clone)]
pub struct InMemoryReviewQueue {
    queue: Arc<Mutex<VecDeque<ReviewRequest>>>,
    max_capacity: usize,
    security_event_store: Option<Arc<dyn SecurityEventStore>>,
}

impl std::fmt::Debug for InMemoryReviewQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryReviewQueue")
            .field("max_capacity", &self.max_capacity)
            .field(
                "security_event_store",
                &self
                    .security_event_store
                    .as_ref()
                    .map(|_| "<SecurityEventStore>"),
            )
            .finish()
    }
}

impl Default for InMemoryReviewQueue {
    fn default() -> Self {
        Self::new(None)
    }
}

impl InMemoryReviewQueue {
    /// Create a new ReviewQueue with default capacity of 1000
    pub fn new(security_event_store: Option<Arc<dyn SecurityEventStore>>) -> Self {
        Self::with_capacity(1000, security_event_store)
    }

    /// Create a new ReviewQueue with specified capacity
    pub fn with_capacity(
        max_capacity: usize,
        security_event_store: Option<Arc<dyn SecurityEventStore>>,
    ) -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            max_capacity,
            security_event_store,
        }
    }
}

#[async_trait::async_trait]
impl ReviewQueue for InMemoryReviewQueue {
    async fn enqueue(&self, review: ReviewRequest) -> anyhow::Result<()> {
        let mut queue = self.queue.lock().await;

        // Enforce capacity limit to prevent DoS attacks
        if queue.len() >= self.max_capacity {
            anyhow::bail!(
                "review queue at capacity: {} (limit: {})",
                queue.len(),
                self.max_capacity
            );
        }

        queue.push_back(review.clone());
        drop(queue);

        if let Some(store) = &self.security_event_store {
            let store = Arc::clone(store);
            let event = SecurityEvent {
                id: cyberclaw_core::ids::SecurityEventId::new(),
                actor: Some(review.requested_by.clone()),
                timestamp: chrono::Utc::now(),
                execution_id: review.execution_id.clone(),
                case_id: review.case_id.clone(),
                node_id: review.owner_node_id.clone(),
                runtime_instance_id: None,
                source: SecurityEventSource::PolicyEngine,
                event_type: SecurityEventType::Custom("ReviewEnqueued".to_string()),
                severity: Severity::Info,
                summary: format!("Review enqueued: {}", review.title),
                details: serde_json::json!({
                    "review_id": review.id.to_string(),
                    "execution_id": review.execution_id.as_ref().map(|id| id.to_string()).unwrap_or_default(),
                    "requested_by_actor_id": review.requested_by.id.as_str(),
                    "requested_by_display_name": review.requested_by.display_name,
                    "review_kind": format!("{:?}", review.review_kind),
                    "title": review.title,
                }),
                trace_id: review.trace_id.clone(),
                credential_evidence: None,
            };
            tokio::spawn(async move {
                let _ = store.store(event).await;
            });
        }

        Ok(())
    }

    async fn list_pending(&self) -> anyhow::Result<Vec<ReviewRequest>> {
        let queue = self.queue.lock().await;
        Ok(queue
            .iter()
            .filter(|r| matches!(r.status, ReviewStatus::Pending))
            .cloned()
            .collect())
    }

    async fn get(&self, review_id: &ReviewId) -> Option<ReviewRequest> {
        let queue = self.queue.lock().await;
        queue.iter().find(|r| &r.id == review_id).cloned()
    }

    async fn approve(&self, review_id: &ReviewId, approver: &ActorRef) -> anyhow::Result<()> {
        let mut queue = self.queue.lock().await;
        let review = queue
            .iter_mut()
            .find(|r| &r.id == review_id)
            .ok_or_else(|| anyhow::anyhow!("review not found: {}", review_id))?;
        review.status = ReviewStatus::Approved;

        if let Some(store) = &self.security_event_store {
            let store = Arc::clone(store);
            // SECURITY FIX (H2): Record actual approver, not requester, for audit trail
            let event = SecurityEvent {
                id: cyberclaw_core::ids::SecurityEventId::new(),
                actor: Some(approver.clone()),
                timestamp: chrono::Utc::now(),
                execution_id: review.execution_id.clone(),
                case_id: review.case_id.clone(),
                node_id: review.owner_node_id.clone(),
                runtime_instance_id: None,
                source: SecurityEventSource::PolicyEngine,
                event_type: SecurityEventType::Custom("ReviewApproved".to_string()),
                severity: Severity::Low,
                summary: format!("Review approved: {}", review.title),
                details: serde_json::json!({
                    "review_id": review.id.to_string(),
                    "execution_id": review.execution_id.as_ref().map(|id| id.to_string()).unwrap_or_default(),
                    "requested_by_actor_id": review.requested_by.id.as_str(),
                    "requested_by_display_name": review.requested_by.display_name,
                    "review_kind": format!("{:?}", review.review_kind),
                    "title": review.title,
                }),
                trace_id: review.trace_id.clone(),
                credential_evidence: None,
            };
            tokio::spawn(async move {
                let _ = store.store(event).await;
            });
        }

        Ok(())
    }

    async fn reject(&self, review_id: &ReviewId, approver: &ActorRef) -> anyhow::Result<()> {
        let mut queue = self.queue.lock().await;
        let review = queue
            .iter_mut()
            .find(|r| &r.id == review_id)
            .ok_or_else(|| anyhow::anyhow!("review not found: {}", review_id))?;
        review.status = ReviewStatus::Rejected;

        if let Some(store) = &self.security_event_store {
            let store = Arc::clone(store);
            // SECURITY FIX (H2): Record actual approver, not requester, for audit trail
            let event = SecurityEvent {
                id: cyberclaw_core::ids::SecurityEventId::new(),
                actor: Some(approver.clone()),
                timestamp: chrono::Utc::now(),
                execution_id: review.execution_id.clone(),
                case_id: review.case_id.clone(),
                node_id: review.owner_node_id.clone(),
                runtime_instance_id: None,
                source: SecurityEventSource::PolicyEngine,
                event_type: SecurityEventType::Custom("ReviewRejected".to_string()),
                severity: Severity::Medium,
                summary: format!("Review rejected: {}", review.title),
                details: serde_json::json!({
                    "review_id": review.id.to_string(),
                    "execution_id": review.execution_id.as_ref().map(|id| id.to_string()).unwrap_or_default(),
                    "requested_by_actor_id": review.requested_by.id.as_str(),
                    "requested_by_display_name": review.requested_by.display_name,
                    "review_kind": format!("{:?}", review.review_kind),
                    "title": review.title,
                }),
                trace_id: review.trace_id.clone(),
                credential_evidence: None,
            };
            tokio::spawn(async move {
                let _ = store.store(event).await;
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::identity::ActorType;
    use cyberclaw_core::ids::ActorId;

    fn create_test_actor(id: &str, display_name: &str) -> ActorRef {
        ActorRef {
            id: ActorId::from_string(id.to_string()).unwrap(),
            actor_type: ActorType::System,
            tenant_id: None,
            home_node_id: None,
            display_name: display_name.to_string(),
        }
    }

    #[tokio::test]
    async fn test_enqueue_and_list_pending() {
        let queue = InMemoryReviewQueue::new(None);
        let review = ReviewRequest::for_execution(
            ReviewId::new(),
            ExecutionId::new(),
            None,
            "Test Review".to_string(),
            "Test summary".to_string(),
            create_test_actor("control-plane", "Control Plane"),
            ReviewKind::Approval,
            TraceId::new(),
            chrono::Utc::now(),
        );

        queue.enqueue(review.clone()).await.unwrap();
        let pending = queue.list_pending().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].title, "Test Review");
    }

    #[tokio::test]
    async fn test_approve_review() {
        let queue = InMemoryReviewQueue::new(None);
        let review_id = ReviewId::new();
        let review = ReviewRequest::for_execution(
            review_id.clone(),
            ExecutionId::new(),
            None,
            "Test Review".to_string(),
            "Test summary".to_string(),
            create_test_actor("control-plane", "Control Plane"),
            ReviewKind::Approval,
            TraceId::new(),
            chrono::Utc::now(),
        );

        queue.enqueue(review).await.unwrap();

        // SECURITY FIX: Pass approver (different from requester) to test audit trail
        let approver = create_test_actor("approver-1", "Test Approver");
        queue.approve(&review_id, &approver).await.unwrap();

        let pending = queue.list_pending().await.unwrap();
        assert_eq!(pending.len(), 0);
    }

    #[tokio::test]
    async fn test_reject_review() {
        let queue = InMemoryReviewQueue::new(None);
        let review_id = ReviewId::new();
        let review = ReviewRequest::for_execution(
            review_id.clone(),
            ExecutionId::new(),
            None,
            "Test Review".to_string(),
            "Test summary".to_string(),
            create_test_actor("control-plane", "Control Plane"),
            ReviewKind::Approval,
            TraceId::new(),
            chrono::Utc::now(),
        );

        queue.enqueue(review).await.unwrap();

        // SECURITY FIX: Pass rejector (different from requester) to test audit trail
        let rejector = create_test_actor("rejector-1", "Test Rejector");
        queue.reject(&review_id, &rejector).await.unwrap();

        let pending = queue.list_pending().await.unwrap();
        assert_eq!(pending.len(), 0);
    }

    #[tokio::test]
    async fn test_capacity_limit_enforced() {
        // Create a queue with small capacity for testing
        let queue = InMemoryReviewQueue::with_capacity(3, None);

        // Fill the queue to capacity
        for i in 0..3 {
            let review = ReviewRequest::for_execution(
                ReviewId::new(),
                ExecutionId::new(),
                None,
                format!("Test Review {}", i),
                "Test summary".to_string(),
                create_test_actor("control-plane", "Control Plane"),
                ReviewKind::Approval,
                TraceId::new(),
                chrono::Utc::now(),
            );
            queue.enqueue(review).await.unwrap();
        }

        // Attempt to add one more should fail
        let overflow_review = ReviewRequest::for_execution(
            ReviewId::new(),
            ExecutionId::new(),
            None,
            "Overflow Review".to_string(),
            "Should be rejected".to_string(),
            create_test_actor("control-plane", "Control Plane"),
            ReviewKind::Approval,
            TraceId::new(),
            chrono::Utc::now(),
        );

        let result = queue.enqueue(overflow_review).await;
        assert!(result.is_err(), "should reject when at capacity");
        assert!(
            result.unwrap_err().to_string().contains("at capacity"),
            "error message should mention capacity"
        );
    }

    #[tokio::test]
    async fn test_capacity_limit_allows_within_limit() {
        let queue = InMemoryReviewQueue::with_capacity(5, None);

        // Add reviews within capacity
        for i in 0..5 {
            let review = ReviewRequest::for_execution(
                ReviewId::new(),
                ExecutionId::new(),
                None,
                format!("Test Review {}", i),
                "Test summary".to_string(),
                create_test_actor("control-plane", "Control Plane"),
                ReviewKind::Approval,
                TraceId::new(),
                chrono::Utc::now(),
            );
            queue
                .enqueue(review)
                .await
                .expect("should succeed within capacity");
        }

        let pending = queue.list_pending().await.unwrap();
        assert_eq!(pending.len(), 5, "all reviews should be queued");
    }
}
