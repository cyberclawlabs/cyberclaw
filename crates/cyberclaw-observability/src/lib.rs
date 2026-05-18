//! Observability infrastructure for CyberClaw execution lifecycle tracking
//!
//! This crate provides:
//! - Tracing spans for execution lifecycle events
//! - Prometheus metrics for performance monitoring
//! - Event recording for audit and debugging, including unified [`SecurityEvent`] support

pub mod distributed;
pub mod events;
pub mod metrics;
pub mod otel_exporter;
pub mod security_event_store;
pub mod token_economics;
pub mod tracing;

pub use self::distributed::{
    correlate_spans, extract_context, extract_trace_context, inject_trace_context,
    propagation_header, ClusterMetrics, DistributedSpan, DistributedTraceContext,
    MetricsAggregator, NodeMetrics, SpanId, SpanStatus, TraceContext, TraceId, TracePropagator,
    TRACEPARENT_HEADER, TRACESTATE_HEADER,
};
pub use self::events::{
    EventRecorder, InMemoryEventRecorder, ObservabilityEvent, RecordError, SecurityEvent,
};
pub use self::metrics::recorders;
pub use self::otel_exporter::{
    ExportError, MetricValue, OtelExporter, OtelExporterConfig, OtelMetricData, OtelProtocol,
    OtelSpanData, OtelSpanStatus,
};
pub use self::security_event_store::{
    EventFilter, InMemorySecurityEventStore, SecurityEventStore, StoreError,
};
pub use self::token_economics::{
    AggregateDimension, InMemoryTokenTracker, TimedExecution, TokenFilter, TokenRecord,
    TokenSummary, TokenTracker, TokenTrackingError,
};
pub use self::tracing::*;
