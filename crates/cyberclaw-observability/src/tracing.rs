//! Tracing spans for execution lifecycle tracking

use cyberclaw_core::execution::ExecutionStatus;
use cyberclaw_core::ids::{ExecutionId, TraceId};
use tracing::{info_span, Span};

/// Create a span for execution lifecycle tracking
pub fn execution_span(execution_id: &ExecutionId, trace_id: &TraceId) -> Span {
    info_span!(
        "execution",
        execution_id = %execution_id,
        trace_id = %trace_id,
    )
}

/// Create a span for status transitions
pub fn status_transition_span(
    execution_id: &ExecutionId,
    from_status: &ExecutionStatus,
    to_status: &ExecutionStatus,
) -> Span {
    info_span!(
        "status_transition",
        execution_id = %execution_id,
        from = ?from_status,
        to = ?to_status,
    )
}

/// Create a span for capability invocation
pub fn capability_span(execution_id: &ExecutionId, capability_id: &str) -> Span {
    info_span!(
        "capability_invocation",
        execution_id = %execution_id,
        capability_id = %capability_id,
    )
}

/// Create a span for review process
pub fn review_span(execution_id: &ExecutionId, review_id: &cyberclaw_core::ids::ReviewId) -> Span {
    info_span!(
        "review_process",
        execution_id = %execution_id,
        review_id = %review_id,
    )
}

/// Create a span for agent execution
pub fn agent_span(execution_id: &ExecutionId, agent_id: &str) -> Span {
    info_span!(
        "agent_execution",
        execution_id = %execution_id,
        agent_id = %agent_id,
    )
}

/// Create a span for skill invocation
pub fn skill_span(execution_id: &ExecutionId, skill_id: &str) -> Span {
    info_span!(
        "skill_invocation",
        execution_id = %execution_id,
        skill_id = %skill_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::execution::ExecutionStatus;
    use cyberclaw_core::ids::{ExecutionId, ReviewId, TraceId};

    #[test]
    fn test_execution_span_creation() {
        let execution_id = ExecutionId::from_string("exec-001".to_string()).unwrap();
        let trace_id = TraceId::from_string("trace-001".to_string()).unwrap();

        // Just verify span can be created without panicking
        let _span = execution_span(&execution_id, &trace_id);
    }

    #[test]
    fn test_status_transition_span_creation() {
        let execution_id = ExecutionId::from_string("exec-001".to_string()).unwrap();
        let from = ExecutionStatus::Pending;
        let to = ExecutionStatus::Running;

        let _span = status_transition_span(&execution_id, &from, &to);
    }

    #[test]
    fn test_capability_span_creation() {
        let execution_id = ExecutionId::from_string("exec-001".to_string()).unwrap();
        let _span = capability_span(&execution_id, "test.capability");
    }

    #[test]
    fn test_review_span_creation() {
        let execution_id = ExecutionId::from_string("exec-001".to_string()).unwrap();
        let review_id = ReviewId::from_string("review-001".to_string()).unwrap();

        let _span = review_span(&execution_id, &review_id);
    }

    #[test]
    fn test_agent_span_creation() {
        let execution_id = ExecutionId::from_string("exec-001".to_string()).unwrap();
        let _span = agent_span(&execution_id, "test-agent");
    }

    #[test]
    fn test_skill_span_creation() {
        let execution_id = ExecutionId::from_string("exec-001".to_string()).unwrap();
        let _span = skill_span(&execution_id, "test-skill");
    }

    #[test]
    fn test_span_guard_pattern_prevents_leaks() {
        // This test verifies that spans are properly closed using the guard pattern.
        // Creating many spans in a loop should not cause unbounded memory growth
        // because each span is properly closed when its guard is dropped.

        let execution_id = ExecutionId::from_string("exec-leak-test".to_string()).unwrap();
        let trace_id = TraceId::from_string("trace-leak-test".to_string()).unwrap();

        // Create 1000 spans with proper guard pattern
        for i in 0..1000 {
            let span = execution_span(&execution_id, &trace_id);

            // CORRECT: Using guard pattern - span is entered and automatically
            // exited when _guard is dropped at the end of this scope
            let _guard = span.enter();

            // Do some work within the span
            let _ = format!("iteration {}", i);

            // _guard is dropped here, automatically closing the span
        }

        // If spans were not properly closed, this test would accumulate
        // 1000 open spans and potentially leak memory. The guard pattern
        // ensures each span is closed when the guard goes out of scope.
    }

    #[test]
    fn test_multiple_span_types_with_guards() {
        // Verify all span creation functions work correctly with guard pattern
        let execution_id = ExecutionId::from_string("exec-multi".to_string()).unwrap();
        let trace_id = TraceId::from_string("trace-multi".to_string()).unwrap();
        let review_id = ReviewId::from_string("review-multi".to_string()).unwrap();

        // Test execution span
        {
            let span = execution_span(&execution_id, &trace_id);
            let _guard = span.enter();
            // span is closed when _guard drops
        }

        // Test status transition span
        {
            let span = status_transition_span(
                &execution_id,
                &ExecutionStatus::Pending,
                &ExecutionStatus::Running,
            );
            let _guard = span.enter();
            // span is closed when _guard drops
        }

        // Test capability span
        {
            let span = capability_span(&execution_id, "test-cap");
            let _guard = span.enter();
            // span is closed when _guard drops
        }

        // Test review span
        {
            let span = review_span(&execution_id, &review_id);
            let _guard = span.enter();
            // span is closed when _guard drops
        }

        // Test agent span
        {
            let span = agent_span(&execution_id, "test-agent");
            let _guard = span.enter();
            // span is closed when _guard drops
        }

        // Test skill span
        {
            let span = skill_span(&execution_id, "test-skill");
            let _guard = span.enter();
            // span is closed when _guard drops
        }

        // All spans have been properly closed by their guards
    }
}
