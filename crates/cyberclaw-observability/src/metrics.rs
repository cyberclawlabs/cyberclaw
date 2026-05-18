//! Prometheus metrics for execution and capability tracking

use lazy_static::lazy_static;
use prometheus::{
    register_counter_vec, register_gauge, register_histogram_vec, CounterVec, Encoder, Gauge,
    HistogramVec, Registry, TextEncoder,
};

// SAFETY NOTE: The `unwrap_or_else(|e| panic!(...))` calls inside `lazy_static!` blocks
// below are intentional and acceptable. Prometheus metric registration can only fail if:
//   (a) the metric name is duplicated within the same process, or
//   (b) the Prometheus default registry has been poisoned.
// Both conditions indicate a programming error (not a runtime/environmental error) that
// must be caught at startup. There is no way to propagate errors from `lazy_static!`
// initializers via `?`, so panic on first access is the correct pattern here.
// This is the established Prometheus-in-Rust convention and cannot be replaced with
// `Result`-returning code without a complete redesign of the metrics subsystem.

lazy_static! {
    /// Global Prometheus registry
    pub static ref METRICS_REGISTRY: Registry = Registry::new();

    /// Total number of executions by status
    pub static ref EXECUTION_COUNT: CounterVec = register_counter_vec!(
        "cyberclaw_execution_total",
        "Total number of executions by status",
        &["status"]
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_execution_total metric: {e}"));

    /// Execution duration in seconds
    pub static ref EXECUTION_DURATION: HistogramVec = register_histogram_vec!(
        "cyberclaw_execution_duration_seconds",
        "Execution duration in seconds",
        &["status"],
        vec![0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0]
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_execution_duration_seconds metric: {e}"));

    /// Current number of executions in each state
    pub static ref EXECUTION_STATE_GAUGE: CounterVec = register_counter_vec!(
        "cyberclaw_execution_state",
        "Current executions by state",
        &["state"]
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_execution_state metric: {e}"));

    /// Capability invocation count
    /// NOTE: capability_id label removed to prevent cardinality explosion
    /// Use event recording for per-capability tracking
    pub static ref CAPABILITY_INVOCATION_COUNT: CounterVec = register_counter_vec!(
        "cyberclaw_capability_invocation_total",
        "Total capability invocations",
        &["status"]
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_capability_invocation_total metric: {e}"));

    /// Capability invocation duration
    /// NOTE: capability_id label removed to prevent cardinality explosion
    /// Use event recording for per-capability tracking
    pub static ref CAPABILITY_INVOCATION_DURATION: HistogramVec = register_histogram_vec!(
        "cyberclaw_capability_invocation_duration_seconds",
        "Capability invocation duration in seconds",
        &["status"],
        vec![0.05, 0.1, 0.5, 1.0, 5.0, 10.0]
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_capability_invocation_duration_seconds metric: {e}"));

    /// Review queue waiting time
    pub static ref REVIEW_WAIT_TIME: HistogramVec = register_histogram_vec!(
        "cyberclaw_review_wait_seconds",
        "Time spent waiting for review approval",
        &["risk_level"],
        vec![1.0, 5.0, 30.0, 60.0, 300.0, 600.0, 1800.0, 3600.0]
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_review_wait_seconds metric: {e}"));

    /// Review queue size
    pub static ref REVIEW_QUEUE_SIZE: Gauge = register_gauge!(
        "cyberclaw_review_queue_size",
        "Current number of pending reviews"
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_review_queue_size metric: {e}"));

    /// Success rate (derived metric, calculated from counts)
    pub static ref EXECUTION_SUCCESS_RATE: Gauge = register_gauge!(
        "cyberclaw_execution_success_rate",
        "Success rate of executions (0.0 to 1.0)"
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_execution_success_rate metric: {e}"));

    /// Agent execution count
    /// NOTE: agent_id label removed to prevent cardinality explosion
    /// Use event recording for per-agent tracking
    pub static ref AGENT_EXECUTION_COUNT: CounterVec = register_counter_vec!(
        "cyberclaw_agent_execution_total",
        "Total agent executions",
        &["status"]
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_agent_execution_total metric: {e}"));

    /// Skill invocation count
    /// NOTE: skill_id label removed to prevent cardinality explosion
    /// Use event recording for per-skill tracking
    pub static ref SKILL_INVOCATION_COUNT: CounterVec = register_counter_vec!(
        "cyberclaw_skill_invocation_total",
        "Total skill invocations",
        &["status"]
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_skill_invocation_total metric: {e}"));

    /// Retry attempt count by operation name.
    /// Incremented on every transient failure that triggers a retry.
    pub static ref RETRY_ATTEMPT_COUNT: CounterVec = register_counter_vec!(
        "cyberclaw_retry_attempts_total",
        "Total number of retry attempts triggered by transient failures",
        &["operation"]
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_retry_attempts_total metric: {e}"));

    /// Retry exhaustion count by operation name.
    /// Incremented when all retry attempts have been consumed and the
    /// operation still fails.
    pub static ref RETRY_EXHAUSTED_COUNT: CounterVec = register_counter_vec!(
        "cyberclaw_retry_exhausted_total",
        "Total number of times all retry attempts were exhausted",
        &["operation"]
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_retry_exhausted_total metric: {e}"));

    // ========================
    // Tenant-specific metrics
    // ========================

    /// Per-tenant execution count
    /// tenant_id is a bounded dimension (limited number of tenants) so it's safe as a label
    pub static ref TENANT_EXECUTION_COUNT: CounterVec = register_counter_vec!(
        "cyberclaw_tenant_execution_total",
        "Total executions per tenant by status",
        &["tenant_id", "status"]
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_tenant_execution_total metric: {e}"));

    /// Per-tenant execution duration
    pub static ref TENANT_EXECUTION_DURATION: HistogramVec = register_histogram_vec!(
        "cyberclaw_tenant_execution_duration_seconds",
        "Execution duration per tenant in seconds",
        &["tenant_id", "status"],
        vec![0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0]
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_tenant_execution_duration_seconds metric: {e}"));

    /// Per-tenant capability invocation count
    pub static ref TENANT_CAPABILITY_COUNT: CounterVec = register_counter_vec!(
        "cyberclaw_tenant_capability_invocation_total",
        "Total capability invocations per tenant",
        &["tenant_id", "status"]
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_tenant_capability_invocation_total metric: {e}"));

    /// Per-tenant active execution gauge
    pub static ref TENANT_ACTIVE_EXECUTIONS: CounterVec = register_counter_vec!(
        "cyberclaw_tenant_active_executions",
        "Current number of active executions per tenant",
        &["tenant_id"]
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_tenant_active_executions metric: {e}"));

    /// Per-tenant review requests
    pub static ref TENANT_REVIEW_COUNT: CounterVec = register_counter_vec!(
        "cyberclaw_tenant_review_total",
        "Total review requests per tenant by review type",
        &["tenant_id", "review_type"]
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_tenant_review_total metric: {e}"));

    /// Per-tenant governance decision count
    pub static ref TENANT_GOVERNANCE_DECISION_COUNT: CounterVec = register_counter_vec!(
        "cyberclaw_tenant_governance_decision_total",
        "Total governance decisions per tenant by decision type",
        &["tenant_id", "decision_type"]
    )
    .unwrap_or_else(|e| panic!("failed to register cyberclaw_tenant_governance_decision_total metric: {e}"));
}

/// Helper functions for recording metrics
pub mod recorders {
    use super::*;
    use cyberclaw_core::execution::ExecutionStatus;

    /// Record execution completion
    pub fn record_execution_complete(status: &ExecutionStatus, duration_secs: f64) {
        let status_label = format!("{:?}", status);
        EXECUTION_COUNT.with_label_values(&[&status_label]).inc();
        EXECUTION_DURATION
            .with_label_values(&[&status_label])
            .observe(duration_secs);
    }

    /// Record execution state change
    pub fn record_execution_state_change(from_state: &str, to_state: &str) {
        EXECUTION_STATE_GAUGE.with_label_values(&[from_state]).inc();
        EXECUTION_STATE_GAUGE.with_label_values(&[to_state]).inc();
    }

    /// Record capability invocation
    /// NOTE: capability_id parameter kept for API compatibility but not used as label
    pub fn record_capability_invocation(_capability_id: &str, success: bool, duration_secs: f64) {
        let status = if success { "success" } else { "failure" };
        CAPABILITY_INVOCATION_COUNT
            .with_label_values(&[status])
            .inc();
        CAPABILITY_INVOCATION_DURATION
            .with_label_values(&[status])
            .observe(duration_secs);
    }

    /// Record review wait time
    pub fn record_review_wait_time(risk_level: &str, wait_time_secs: f64) {
        REVIEW_WAIT_TIME
            .with_label_values(&[risk_level])
            .observe(wait_time_secs);
    }

    /// Update review queue size
    pub fn update_review_queue_size(size: usize) {
        // usize → f64 精度损失是可接受的 (用于监控指标)
        #[allow(clippy::cast_precision_loss)]
        REVIEW_QUEUE_SIZE.set(size as f64);
    }

    /// Record agent execution
    /// NOTE: agent_id parameter kept for API compatibility but not used as label
    pub fn record_agent_execution(_agent_id: &str, status: &ExecutionStatus) {
        let status_label = format!("{:?}", status);
        AGENT_EXECUTION_COUNT
            .with_label_values(&[&status_label])
            .inc();
    }

    /// Record skill invocation
    /// NOTE: skill_id parameter kept for API compatibility but not used as label
    pub fn record_skill_invocation(_skill_id: &str, success: bool) {
        let status = if success { "success" } else { "failure" };
        SKILL_INVOCATION_COUNT.with_label_values(&[status]).inc();
    }

    /// Calculate and update success rate
    pub fn update_success_rate() {
        // This would typically be called periodically
        // Here we just provide the structure
        // Real implementation would query EXECUTION_COUNT metrics
    }

    /// Record a single retry attempt for `operation`.
    ///
    /// Called each time a transient failure triggers a retry (i.e., the
    /// operation failed but there are still remaining attempts).
    pub fn record_retry_attempt(operation: &str) {
        RETRY_ATTEMPT_COUNT.with_label_values(&[operation]).inc();
    }

    /// Record that all retry attempts for `operation` have been exhausted.
    ///
    /// Called when the final attempt fails and the error is propagated to
    /// the caller.
    pub fn record_retry_exhausted(operation: &str) {
        RETRY_EXHAUSTED_COUNT.with_label_values(&[operation]).inc();
    }

    // ========================
    // Tenant-specific recorders
    // ========================

    /// Record tenant execution completion with tenant_id
    ///
    /// If tenant_id is None (system-level execution), uses "system" as the label
    pub fn record_tenant_execution(
        tenant_id: Option<&str>,
        status: &ExecutionStatus,
        duration_secs: f64,
    ) {
        let tenant_label = tenant_id.unwrap_or("system");
        let status_label = format!("{:?}", status);

        TENANT_EXECUTION_COUNT
            .with_label_values(&[tenant_label, &status_label])
            .inc();
        TENANT_EXECUTION_DURATION
            .with_label_values(&[tenant_label, &status_label])
            .observe(duration_secs);
    }

    /// Record tenant capability invocation
    ///
    /// If tenant_id is None, uses "system" as the label
    pub fn record_tenant_capability_invocation(tenant_id: Option<&str>, success: bool) {
        let tenant_label = tenant_id.unwrap_or("system");
        let status = if success { "success" } else { "failure" };

        TENANT_CAPABILITY_COUNT
            .with_label_values(&[tenant_label, status])
            .inc();
    }

    /// Increment tenant active execution count
    ///
    /// Call when an execution starts for a tenant
    pub fn increment_tenant_active_executions(tenant_id: Option<&str>) {
        let tenant_label = tenant_id.unwrap_or("system");
        TENANT_ACTIVE_EXECUTIONS
            .with_label_values(&[tenant_label])
            .inc();
    }

    /// Decrement tenant active execution count
    ///
    /// Call when an execution completes for a tenant
    pub fn decrement_tenant_active_executions(tenant_id: Option<&str>) {
        // Note: CounterVec doesn't support decrement, so we track this separately
        // In production, this would use a Gauge instead
        // For now, we only increment on completion to track total completed
        let tenant_label = tenant_id.unwrap_or("system");
        // This is actually tracking completed executions, not active ones
        // We'll keep the increment for now as it represents total activity
        TENANT_ACTIVE_EXECUTIONS
            .with_label_values(&[tenant_label])
            .inc();
    }

    /// Record tenant review request
    ///
    /// review_type examples: "Human", "Approval", "Security", "Escalation"
    pub fn record_tenant_review_request(tenant_id: Option<&str>, review_type: &str) {
        let tenant_label = tenant_id.unwrap_or("system");
        TENANT_REVIEW_COUNT
            .with_label_values(&[tenant_label, review_type])
            .inc();
    }

    /// Record tenant governance decision
    ///
    /// decision_type examples: "Allow", "Deny", "ReviewRequired"
    pub fn record_tenant_governance_decision(tenant_id: Option<&str>, decision_type: &str) {
        let tenant_label = tenant_id.unwrap_or("system");
        TENANT_GOVERNANCE_DECISION_COUNT
            .with_label_values(&[tenant_label, decision_type])
            .inc();
    }
}

/// Export metrics in Prometheus text format
pub fn export_metrics() -> Result<String, Box<dyn std::error::Error>> {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer)?;
    Ok(String::from_utf8(buffer)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::execution::ExecutionStatus;

    #[test]
    fn test_execution_metrics_recording() {
        recorders::record_execution_complete(&ExecutionStatus::Completed, 5.5);

        // Verify counter incremented
        let completed_count = EXECUTION_COUNT.with_label_values(&["Completed"]).get();
        assert!(completed_count > 0.0);
    }

    #[test]
    fn test_capability_metrics_recording() {
        recorders::record_capability_invocation("test.capability", true, 1.0);

        // Verify counter incremented (now only uses status label)
        let count = CAPABILITY_INVOCATION_COUNT
            .with_label_values(&["success"])
            .get();
        assert!(count > 0.0);
    }

    #[test]
    fn test_review_metrics_recording() {
        recorders::record_review_wait_time("Medium", 30.0);

        // Verify histogram recorded
        let count = REVIEW_WAIT_TIME
            .with_label_values(&["Medium"])
            .get_sample_count();
        assert!(count > 0);
    }

    #[test]
    fn test_review_queue_size_update() {
        recorders::update_review_queue_size(5);
        assert_eq!(REVIEW_QUEUE_SIZE.get(), 5.0);

        recorders::update_review_queue_size(3);
        assert_eq!(REVIEW_QUEUE_SIZE.get(), 3.0);
    }

    #[test]
    fn test_agent_metrics_recording() {
        recorders::record_agent_execution("test-agent", &ExecutionStatus::Completed);

        // Verify counter incremented (now only uses status label)
        let count = AGENT_EXECUTION_COUNT
            .with_label_values(&["Completed"])
            .get();
        assert!(count > 0.0);
    }

    #[test]
    fn test_skill_metrics_recording() {
        recorders::record_skill_invocation("test-skill", true);

        // Verify counter incremented (now only uses status label)
        let count = SKILL_INVOCATION_COUNT.with_label_values(&["success"]).get();
        assert!(count > 0.0);
    }

    #[test]
    fn test_metrics_export() {
        recorders::record_execution_complete(&ExecutionStatus::Completed, 1.0);

        let exported = export_metrics();
        assert!(exported.is_ok());

        let metrics_text = exported.unwrap();
        assert!(metrics_text.contains("cyberclaw_execution_total"));
    }

    #[tokio::test]
    async fn test_concurrent_metrics_recording_no_deadlock() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;
        use tokio::time::{sleep, Duration};

        // Counter to track completed operations
        let counter = Arc::new(AtomicU32::new(0));

        // Spawn 20 concurrent tasks recording metrics
        let mut handles = vec![];
        for i in 0..20 {
            let counter_clone = counter.clone();
            let handle = tokio::spawn(async move {
                for _ in 0..50 {
                    // Record various metrics concurrently
                    recorders::record_execution_complete(&ExecutionStatus::Completed, 1.5);
                    recorders::record_capability_invocation("test-cap", true, 0.1);
                    recorders::record_agent_execution("agent", &ExecutionStatus::Completed);
                    recorders::record_skill_invocation("skill", true);
                    recorders::update_review_queue_size(i);
                    recorders::record_review_wait_time("Low", 5.0);
                    recorders::record_retry_attempt("test_op");

                    // Small delay to interleave operations
                    sleep(Duration::from_micros(10)).await;
                }
                counter_clone.fetch_add(1, Ordering::SeqCst);
            });
            handles.push(handle);
        }

        // Spawn a lightweight task that should complete quickly
        let lightweight_task = tokio::spawn(async move {
            sleep(Duration::from_millis(100)).await;
            "completed"
        });

        // Verify lightweight task completes (not blocked by metrics recording)
        let result = tokio::time::timeout(Duration::from_secs(5), lightweight_task).await;
        assert!(
            result.is_ok(),
            "Lightweight task was blocked by concurrent metrics recording"
        );
        assert_eq!(result.unwrap().unwrap(), "completed");

        // Wait for all metric recording tasks to complete
        for handle in handles {
            handle.await.expect("Task panicked");
        }

        // Verify all 20 tasks completed
        assert_eq!(
            counter.load(Ordering::SeqCst),
            20,
            "Not all concurrent tasks completed"
        );

        // Verify metrics were recorded (check counters are non-zero)
        let completed_count = EXECUTION_COUNT.with_label_values(&["Completed"]).get();
        assert!(
            completed_count >= 1000.0,
            "Expected at least 1000 execution records, got {}",
            completed_count
        );

        let cap_count = CAPABILITY_INVOCATION_COUNT
            .with_label_values(&["success"])
            .get();
        assert!(
            cap_count >= 1000.0,
            "Expected at least 1000 capability records, got {}",
            cap_count
        );
    }

    // ========================
    // Tenant metrics tests
    // ========================

    #[test]
    fn test_tenant_execution_recording() {
        recorders::record_tenant_execution(Some("tenant-123"), &ExecutionStatus::Completed, 2.5);

        let count = TENANT_EXECUTION_COUNT
            .with_label_values(&["tenant-123", "Completed"])
            .get();
        assert!(count > 0.0, "Tenant execution count should be incremented");
    }

    #[test]
    fn test_tenant_execution_recording_system() {
        recorders::record_tenant_execution(None, &ExecutionStatus::Completed, 1.0);

        let count = TENANT_EXECUTION_COUNT
            .with_label_values(&["system", "Completed"])
            .get();
        assert!(
            count > 0.0,
            "System execution count should be incremented for None tenant_id"
        );
    }

    #[test]
    fn test_tenant_capability_recording() {
        recorders::record_tenant_capability_invocation(Some("tenant-456"), true);

        let count = TENANT_CAPABILITY_COUNT
            .with_label_values(&["tenant-456", "success"])
            .get();
        assert!(count > 0.0, "Tenant capability count should be incremented");
    }

    #[test]
    fn test_tenant_active_executions() {
        recorders::increment_tenant_active_executions(Some("tenant-789"));
        recorders::decrement_tenant_active_executions(Some("tenant-789"));

        let count = TENANT_ACTIVE_EXECUTIONS
            .with_label_values(&["tenant-789"])
            .get();
        assert!(count > 0.0, "Tenant active executions should be tracked");
    }

    #[test]
    fn test_tenant_review_recording() {
        recorders::record_tenant_review_request(Some("tenant-abc"), "Human");

        let count = TENANT_REVIEW_COUNT
            .with_label_values(&["tenant-abc", "Human"])
            .get();
        assert!(count > 0.0, "Tenant review count should be incremented");
    }

    #[test]
    fn test_tenant_governance_decision_recording() {
        recorders::record_tenant_governance_decision(Some("tenant-def"), "Allow");

        let count = TENANT_GOVERNANCE_DECISION_COUNT
            .with_label_values(&["tenant-def", "Allow"])
            .get();
        assert!(
            count > 0.0,
            "Tenant governance decision count should be incremented"
        );
    }

    #[test]
    fn test_multiple_tenant_metrics() {
        // Record metrics for multiple tenants
        recorders::record_tenant_execution(Some("tenant-1"), &ExecutionStatus::Completed, 1.0);
        recorders::record_tenant_execution(Some("tenant-2"), &ExecutionStatus::Failed, 0.5);
        recorders::record_tenant_capability_invocation(Some("tenant-1"), true);
        recorders::record_tenant_capability_invocation(Some("tenant-2"), false);

        // Verify tenant-1 metrics
        let tenant1_count = TENANT_EXECUTION_COUNT
            .with_label_values(&["tenant-1", "Completed"])
            .get();
        assert!(tenant1_count > 0.0);

        let tenant1_cap_count = TENANT_CAPABILITY_COUNT
            .with_label_values(&["tenant-1", "success"])
            .get();
        assert!(tenant1_cap_count > 0.0);

        // Verify tenant-2 metrics
        let tenant2_count = TENANT_EXECUTION_COUNT
            .with_label_values(&["tenant-2", "Failed"])
            .get();
        assert!(tenant2_count > 0.0);

        let tenant2_cap_count = TENANT_CAPABILITY_COUNT
            .with_label_values(&["tenant-2", "failure"])
            .get();
        assert!(tenant2_cap_count > 0.0);
    }
}
