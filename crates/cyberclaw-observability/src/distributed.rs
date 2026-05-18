//! Distributed tracing and metrics aggregation for multi-node CyberClaw clusters.
//!
//! This module provides:
//! - [`DistributedTraceContext`]: Cross-node trace propagation context
//! - [`TraceId`] / [`SpanId`]: Globally unique 128-bit identifiers
//! - [`DistributedSpan`]: Per-node span record with timing and status
//! - [`NodeMetrics`]: Per-node metrics snapshot
//! - [`MetricsAggregator`]: Aggregates metrics from multiple nodes
//! - [`ClusterMetrics`]: Cluster-wide aggregated view
//! - [`propagation_header`] / [`extract_context`]: HTTP header serialization

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

// ─── TraceId / SpanId ─────────────────────────────────────────────────────────

/// A globally unique 128-bit trace identifier, encoded as a lowercase hex string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TraceId(String);

/// A 128-bit span identifier scoped to a single trace, encoded as a lowercase hex string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpanId(String);

fn random_hex_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Simple deterministic-enough ID: combine timestamp nanos with a counter.
    // Avoids adding uuid/rand as a direct dependency beyond what is in dev-deps.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Mix bits to reduce collision chance within a process
    let hi =
        (u64::from(ts) ^ (seq.wrapping_mul(6364136223846793005))).wrapping_add(1442695040888963407);
    let lo = seq
        .wrapping_mul(2862933555777941757)
        .wrapping_add(u64::from(ts).wrapping_mul(3935559000370003845));
    format!("{:016x}{:016x}", hi, lo)
}

impl TraceId {
    /// Generate a new unique [`TraceId`].
    pub fn new() -> Self {
        Self(random_hex_id())
    }

    /// Create a [`TraceId`] from a pre-existing hex string (e.g. received from a peer).
    pub fn from_raw(s: &str) -> Self {
        Self(s.to_string())
    }

    /// Return the inner hex string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for TraceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TraceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl SpanId {
    /// Generate a new unique [`SpanId`].
    pub fn new() -> Self {
        Self(random_hex_id())
    }

    /// Create a [`SpanId`] from a pre-existing hex string.
    pub fn from_raw(s: &str) -> Self {
        Self(s.to_string())
    }

    /// Return the inner hex string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SpanId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SpanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ─── DistributedTraceContext ───────────────────────────────────────────────────

/// Cross-node trace propagation context.
///
/// Carries the identifiers needed to correlate spans across multiple nodes in
/// a CyberClaw cluster. Propagated via HTTP headers between control-plane and
/// connector/agent calls.
///
/// # Wire Format
///
/// Serialized by [`propagation_header`] / deserialized by [`extract_context`]
/// as a base64-encoded JSON blob (single header value, no per-field headers).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistributedTraceContext {
    /// Global trace ID that is stable across all nodes for one logical operation.
    pub trace_id: TraceId,
    /// Span ID for the current processing unit on `current_node_id`.
    pub span_id: SpanId,
    /// Span ID of the parent span, if any (absent at the root).
    pub parent_span_id: Option<SpanId>,
    /// Node ID of the node that originated this trace.
    pub origin_node_id: String,
    /// Node ID currently processing this request.
    pub current_node_id: String,
}

impl DistributedTraceContext {
    /// Create a new root trace context originating from `node_id`.
    pub fn new_root(node_id: impl Into<String>) -> Self {
        let node_id = node_id.into();
        Self {
            trace_id: TraceId::new(),
            span_id: SpanId::new(),
            parent_span_id: None,
            origin_node_id: node_id.clone(),
            current_node_id: node_id,
        }
    }

    /// Derive a child context for `next_node_id`.
    ///
    /// The `trace_id` is preserved; the current `span_id` becomes the
    /// `parent_span_id` and a new `span_id` is allocated.
    pub fn child(&self, next_node_id: impl Into<String>) -> Self {
        Self {
            trace_id: self.trace_id.clone(),
            span_id: SpanId::new(),
            parent_span_id: Some(self.span_id.clone()),
            origin_node_id: self.origin_node_id.clone(),
            current_node_id: next_node_id.into(),
        }
    }
}

// ─── Propagation header helpers ───────────────────────────────────────────────

/// Serialize a [`DistributedTraceContext`] into a single HTTP header value.
///
/// Format: `cyberclaw-trace=<base64(json)>`
pub fn propagation_header(ctx: &DistributedTraceContext) -> String {
    // Serialize to compact JSON then base64-encode to keep it header-safe.
    let json = serde_json::to_string(ctx).unwrap_or_default();
    // Manual base64 encoding without an additional dependency.
    let encoded = base64_encode(json.as_bytes());
    format!("cyberclaw-trace={}", encoded)
}

/// Deserialize a [`DistributedTraceContext`] from a header value produced by
/// [`propagation_header`].
///
/// Returns `None` on any parse or decode failure.
pub fn extract_context(header: &str) -> Option<DistributedTraceContext> {
    let value = header.strip_prefix("cyberclaw-trace=")?;
    let bytes = base64_decode(value)?;
    let json = std::str::from_utf8(&bytes).ok()?;
    serde_json::from_str(json).ok()
}

// ─── Minimal base64 (no external dep) ────────────────────────────────────────

const B64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(B64_CHARS[((n >> 18) & 0x3f) as usize] as char);
        out.push(B64_CHARS[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64_CHARS[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64_CHARS[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    // Build a reverse lookup table.
    let mut table = [0xffu8; 256];
    for (i, &c) in B64_CHARS.iter().enumerate() {
        table[c as usize] = i as u8;
    }
    let input = input.as_bytes();
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut i = 0;
    while i + 3 < input.len() {
        let a = table[input[i] as usize];
        let b = table[input[i + 1] as usize];
        let c = if input[i + 2] == b'=' {
            0
        } else {
            table[input[i + 2] as usize]
        };
        let d = if input[i + 3] == b'=' {
            0
        } else {
            table[input[i + 3] as usize]
        };
        if a == 0xff || b == 0xff || c == 0xff || d == 0xff {
            return None;
        }
        out.push((a << 2) | (b >> 4));
        if input[i + 2] != b'=' {
            out.push((b << 4) | (c >> 2));
        }
        if input[i + 3] != b'=' {
            out.push((c << 6) | d);
        }
        i += 4;
    }
    Some(out)
}

// ─── SpanStatus ───────────────────────────────────────────────────────────────

/// Completion status of a distributed span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpanStatus {
    /// Still in progress.
    InProgress,
    /// Completed successfully.
    Ok,
    /// Completed with an error.
    Error(String),
}

// ─── DistributedSpan ──────────────────────────────────────────────────────────

/// A cross-node span record that captures timing and status for a single unit
/// of work on one node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributedSpan {
    /// ID of the node that recorded this span.
    pub node_id: String,
    /// Human-readable operation name (e.g. `"execute_capability"`).
    pub operation: String,
    /// Wall-clock time when the span started.
    pub start_time: DateTime<Utc>,
    /// Wall-clock time when the span ended; `None` while in progress.
    pub end_time: Option<DateTime<Utc>>,
    /// Completion status.
    pub status: SpanStatus,
    /// Arbitrary key/value metadata attached to this span.
    pub attributes: HashMap<String, String>,
}

impl DistributedSpan {
    /// Create a new in-progress span on `node_id` for `operation`.
    pub fn start(node_id: impl Into<String>, operation: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            operation: operation.into(),
            start_time: Utc::now(),
            end_time: None,
            status: SpanStatus::InProgress,
            attributes: HashMap::new(),
        }
    }

    /// Mark the span as completed successfully.
    pub fn finish_ok(&mut self) {
        self.end_time = Some(Utc::now());
        self.status = SpanStatus::Ok;
    }

    /// Mark the span as completed with an error.
    pub fn finish_error(&mut self, message: impl Into<String>) {
        self.end_time = Some(Utc::now());
        self.status = SpanStatus::Error(message.into());
    }

    /// Attach a key/value attribute to this span.
    pub fn set_attribute(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.attributes.insert(key.into(), value.into());
    }

    /// Duration in milliseconds, or `None` if not yet finished.
    pub fn duration_ms(&self) -> Option<i64> {
        self.end_time
            .map(|end| (end - self.start_time).num_milliseconds())
    }
}

// ─── NodeMetrics ──────────────────────────────────────────────────────────────

/// Snapshot of per-node operational metrics at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMetrics {
    /// Unique identifier of the reporting node.
    pub node_id: String,
    /// Timestamp of this snapshot.
    pub timestamp: DateTime<Utc>,
    /// Number of currently active (in-progress) executions.
    pub active_executions: u64,
    /// Total completed executions since node start.
    pub completed_count: u64,
    /// Total errored executions since node start.
    pub error_count: u64,
    /// Rolling average execution latency in milliseconds.
    pub avg_latency_ms: f64,
}

impl NodeMetrics {
    /// Create a new metrics snapshot for `node_id`.
    pub fn new(
        node_id: impl Into<String>,
        active_executions: u64,
        completed_count: u64,
        error_count: u64,
        avg_latency_ms: f64,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            timestamp: Utc::now(),
            active_executions,
            completed_count,
            error_count,
            avg_latency_ms,
        }
    }
}

// ─── ClusterMetrics ───────────────────────────────────────────────────────────

/// Cluster-wide aggregated metrics derived from [`NodeMetrics`] snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterMetrics {
    /// Sum of `active_executions` across all nodes.
    pub total_active: u64,
    /// Sum of `completed_count` across all nodes.
    pub total_completed: u64,
    /// Sum of `error_count` across all nodes.
    pub total_errors: u64,
    /// Number of nodes that contributed to this snapshot.
    pub node_count: usize,
    /// Weighted average of per-node `avg_latency_ms` (equal weights).
    pub avg_cluster_latency: f64,
}

// ─── MetricsAggregator ────────────────────────────────────────────────────────

/// Collects per-node [`NodeMetrics`] snapshots and produces a [`ClusterMetrics`]
/// aggregate.
///
/// The latest snapshot per node is retained; older snapshots for the same node
/// are replaced.
///
/// Thread-safe via an internal [`RwLock`].
#[derive(Debug, Clone, Default)]
pub struct MetricsAggregator {
    // keyed by node_id, latest snapshot wins
    snapshots: Arc<RwLock<HashMap<String, NodeMetrics>>>,
}

impl MetricsAggregator {
    /// Create a new empty aggregator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record (or replace) the latest metrics snapshot for the node identified
    /// by `node_metrics.node_id`.
    pub fn record(&self, node_metrics: NodeMetrics) {
        let mut guard = self
            .snapshots
            .write()
            .expect("MetricsAggregator lock poisoned");
        guard.insert(node_metrics.node_id.clone(), node_metrics);
    }

    /// Aggregate all recorded node snapshots into a single [`ClusterMetrics`].
    ///
    /// Returns a zero-valued [`ClusterMetrics`] if no snapshots have been
    /// recorded yet.
    pub fn aggregate(&self) -> ClusterMetrics {
        let guard = self
            .snapshots
            .read()
            .expect("MetricsAggregator lock poisoned");
        let nodes: Vec<&NodeMetrics> = guard.values().collect();
        let node_count = nodes.len();
        if node_count == 0 {
            return ClusterMetrics {
                total_active: 0,
                total_completed: 0,
                total_errors: 0,
                node_count: 0,
                avg_cluster_latency: 0.0,
            };
        }
        let total_active = nodes.iter().map(|n| n.active_executions).sum();
        let total_completed = nodes.iter().map(|n| n.completed_count).sum();
        let total_errors = nodes.iter().map(|n| n.error_count).sum();
        let sum_latency: f64 = nodes.iter().map(|n| n.avg_latency_ms).sum();
        #[allow(clippy::cast_precision_loss)]
        let avg_cluster_latency = sum_latency / node_count as f64;
        ClusterMetrics {
            total_active,
            total_completed,
            total_errors,
            node_count,
            avg_cluster_latency,
        }
    }

    /// Return the number of nodes currently tracked.
    pub fn node_count(&self) -> usize {
        self.snapshots
            .read()
            .expect("MetricsAggregator lock poisoned")
            .len()
    }
}

// ─── W3C Trace Context ───────────────────────────────────────────────────────

/// W3C Trace Context representation for cross-service propagation.
///
/// Carries the standard fields from the `traceparent` and `tracestate` headers
/// as defined by the W3C Trace Context specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceContext {
    /// 32-char lowercase hex trace identifier.
    pub trace_id: String,
    /// 16-char lowercase hex span identifier.
    pub span_id: String,
    /// Parent span identifier, if this is a child span.
    pub parent_span_id: Option<String>,
    /// W3C trace flags (8-bit). `0x01` = sampled.
    pub trace_flags: u8,
    /// Optional W3C `tracestate` header value.
    pub trace_state: Option<String>,
}

impl TraceContext {
    /// Create a new root trace context with fresh IDs.
    pub fn new_root() -> Self {
        Self {
            trace_id: random_hex_id(),
            span_id: random_span_id(),
            parent_span_id: None,
            trace_flags: 0x01, // sampled by default
            trace_state: None,
        }
    }
}

/// Generate a 16-character hex span ID (64 bits).
fn random_span_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    static SPAN_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let seq = SPAN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mixed =
        (u64::from(ts) ^ (seq.wrapping_mul(6364136223846793005))).wrapping_add(1442695040888963407);
    format!("{:016x}", mixed)
}

// ─── W3C Trace Context Propagation ──────────────────────────────────────────

/// Header name for W3C traceparent.
pub const TRACEPARENT_HEADER: &str = "traceparent";
/// Header name for W3C tracestate.
pub const TRACESTATE_HEADER: &str = "tracestate";

/// Inject a [`DistributedSpan`] into W3C `traceparent` / `tracestate` headers.
///
/// Returns a map containing the `traceparent` header and, if present in the
/// span attributes, a `tracestate` header.
///
/// Format: `traceparent: 00-{trace_id}-{span_id}-{flags}`
pub fn inject_trace_context(span: &DistributedSpan) -> HashMap<String, String> {
    let mut headers = HashMap::new();

    let trace_id = span
        .attributes
        .get("trace_id")
        .cloned()
        .unwrap_or_else(|| format!("{:032x}", 0));
    let span_id = span
        .attributes
        .get("span_id")
        .cloned()
        .unwrap_or_else(|| format!("{:016x}", 0));
    let flags: u8 = span
        .attributes
        .get("trace_flags")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x01);

    headers.insert(
        TRACEPARENT_HEADER.to_string(),
        format!("00-{}-{}-{:02x}", trace_id, span_id, flags),
    );

    if let Some(state) = span.attributes.get("trace_state") {
        if !state.is_empty() {
            headers.insert(TRACESTATE_HEADER.to_string(), state.clone());
        }
    }

    headers
}

/// Extract a [`TraceContext`] from W3C `traceparent` / `tracestate` HTTP headers.
///
/// Returns `None` if the `traceparent` header is missing or malformed.
pub fn extract_trace_context(headers: &HashMap<String, String>) -> Option<TraceContext> {
    let traceparent = headers.get(TRACEPARENT_HEADER)?;
    let parts: Vec<&str> = traceparent.split('-').collect();
    if parts.len() != 4 {
        return None;
    }

    let version = parts[0];
    if version != "00" {
        return None;
    }

    let trace_id = parts[1];
    let span_id = parts[2];
    let flags_str = parts[3];

    // Validate hex lengths: trace_id = 32 chars, span_id = 16 chars, flags = 2 chars
    if trace_id.len() != 32 || span_id.len() != 16 || flags_str.len() != 2 {
        return None;
    }

    // Validate hex characters
    if !trace_id.chars().all(|c| c.is_ascii_hexdigit())
        || !span_id.chars().all(|c| c.is_ascii_hexdigit())
        || !flags_str.chars().all(|c| c.is_ascii_hexdigit())
    {
        return None;
    }

    let trace_flags = u8::from_str_radix(flags_str, 16).ok()?;

    let trace_state = headers.get(TRACESTATE_HEADER).cloned();

    Some(TraceContext {
        trace_id: trace_id.to_string(),
        span_id: span_id.to_string(),
        parent_span_id: None,
        trace_flags,
        trace_state,
    })
}

// ─── TracePropagator ─────────────────────────────────────────────────────────

/// Handles injection and extraction of W3C Trace Context headers, and
/// creation of child spans for distributed trace propagation.
#[derive(Debug, Clone, Default)]
pub struct TracePropagator;

impl TracePropagator {
    /// Create a new propagator.
    pub fn new() -> Self {
        Self
    }

    /// Inject trace context from a [`DistributedSpan`] into outgoing HTTP headers.
    ///
    /// Adds `traceparent` (and optionally `tracestate`) entries to `headers`.
    pub fn inject(&self, span: &DistributedSpan, headers: &mut HashMap<String, String>) {
        let trace_headers = inject_trace_context(span);
        headers.extend(trace_headers);
    }

    /// Extract a [`TraceContext`] from incoming HTTP headers.
    ///
    /// Returns `None` if no valid `traceparent` header is present.
    pub fn extract(&self, headers: &HashMap<String, String>) -> Option<TraceContext> {
        extract_trace_context(headers)
    }

    /// Create a child [`DistributedSpan`] linked to a parent [`TraceContext`].
    ///
    /// The child span inherits the parent's `trace_id` and records the parent's
    /// `span_id` as `parent_span_id`. A new `span_id` is generated.
    pub fn create_child_span(&self, parent: &TraceContext, operation: &str) -> DistributedSpan {
        let child_span_id = random_span_id();
        let mut span = DistributedSpan::start("local", operation);
        span.set_attribute("trace_id", &parent.trace_id);
        span.set_attribute("span_id", &child_span_id);
        span.set_attribute("parent_span_id", &parent.span_id);
        span.set_attribute("trace_flags", parent.trace_flags.to_string());
        if let Some(ref state) = parent.trace_state {
            span.set_attribute("trace_state", state);
        }
        span
    }
}

// ─── Cross-node span correlation ────────────────────────────────────────────

/// Verify that a local [`DistributedSpan`] belongs to the same trace as
/// `remote_trace_id`.
///
/// Returns `true` if the span's `trace_id` attribute matches the remote trace ID.
pub fn correlate_spans(local: &DistributedSpan, remote_trace_id: &str) -> bool {
    local
        .attributes
        .get("trace_id")
        .map(|id| id == remote_trace_id)
        .unwrap_or(false)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Test 1: TraceContext creation and child propagation ────────────────────

    #[test]
    fn test_trace_context_creation_and_propagation() {
        let root = DistributedTraceContext::new_root("node-a");

        assert_eq!(root.origin_node_id, "node-a");
        assert_eq!(root.current_node_id, "node-a");
        assert!(root.parent_span_id.is_none(), "root has no parent");

        let child = root.child("node-b");

        // trace_id is preserved
        assert_eq!(child.trace_id, root.trace_id);
        // parent_span_id is the root's span_id
        assert_eq!(child.parent_span_id.as_ref(), Some(&root.span_id));
        // a new span_id was allocated
        assert_ne!(child.span_id, root.span_id);
        // origin is still node-a, current is node-b
        assert_eq!(child.origin_node_id, "node-a");
        assert_eq!(child.current_node_id, "node-b");
    }

    // ── Test 2: Header serialization / deserialization round-trip ─────────────

    #[test]
    fn test_propagation_header_round_trip() {
        let ctx = DistributedTraceContext::new_root("node-origin");
        let child = ctx.child("node-remote");

        let header = propagation_header(&child);
        assert!(
            header.starts_with("cyberclaw-trace="),
            "header must start with 'cyberclaw-trace='"
        );

        let recovered = extract_context(&header).expect("extract_context must succeed");

        assert_eq!(recovered.trace_id, child.trace_id);
        assert_eq!(recovered.span_id, child.span_id);
        assert_eq!(recovered.parent_span_id, child.parent_span_id);
        assert_eq!(recovered.origin_node_id, child.origin_node_id);
        assert_eq!(recovered.current_node_id, child.current_node_id);
    }

    // ── Test 3: extract_context returns None on bad input ─────────────────────

    #[test]
    fn test_extract_context_bad_input_returns_none() {
        assert!(extract_context("").is_none());
        assert!(extract_context("cyberclaw-trace=!!!not-base64!!!").is_none());
        assert!(extract_context("other-header=abc").is_none());
    }

    // ── Test 4: DistributedSpan recording ─────────────────────────────────────

    #[test]
    fn test_distributed_span_recording() {
        let mut span = DistributedSpan::start("node-x", "execute_capability");
        span.set_attribute("capability_id", "file:read");

        assert_eq!(span.node_id, "node-x");
        assert_eq!(span.operation, "execute_capability");
        assert_eq!(span.status, SpanStatus::InProgress);
        assert!(span.end_time.is_none());
        assert_eq!(
            span.attributes.get("capability_id").map(String::as_str),
            Some("file:read")
        );

        span.finish_ok();

        assert_eq!(span.status, SpanStatus::Ok);
        assert!(span.end_time.is_some());
        assert!(span.duration_ms().is_some());
    }

    #[test]
    fn test_distributed_span_finish_error() {
        let mut span = DistributedSpan::start("node-y", "invoke_skill");
        span.finish_error("timeout waiting for skill response");

        assert!(matches!(span.status, SpanStatus::Error(_)));
        if let SpanStatus::Error(msg) = &span.status {
            assert!(msg.contains("timeout"));
        }
    }

    // ── Test 5: MetricsAggregator multi-node aggregation ──────────────────────

    #[test]
    fn test_metrics_aggregator_multi_node() {
        let agg = MetricsAggregator::new();

        agg.record(NodeMetrics::new("node-1", 3, 100, 5, 120.0));
        agg.record(NodeMetrics::new("node-2", 7, 200, 10, 80.0));
        agg.record(NodeMetrics::new("node-3", 1, 50, 2, 200.0));

        assert_eq!(agg.node_count(), 3);

        let cluster = agg.aggregate();

        assert_eq!(cluster.node_count, 3);
        assert_eq!(cluster.total_active, 3 + 7 + 1);
        assert_eq!(cluster.total_completed, 100 + 200 + 50);
        assert_eq!(cluster.total_errors, 5 + 10 + 2);

        // avg_cluster_latency = (120 + 80 + 200) / 3 = 133.333...
        let expected_latency = (120.0 + 80.0 + 200.0) / 3.0;
        assert!(
            (cluster.avg_cluster_latency - expected_latency).abs() < 1e-9,
            "latency mismatch: {} vs {}",
            cluster.avg_cluster_latency,
            expected_latency
        );
    }

    // ── Test 6: ClusterMetrics zero-node case ─────────────────────────────────

    #[test]
    fn test_cluster_metrics_no_nodes() {
        let agg = MetricsAggregator::new();
        let cluster = agg.aggregate();

        assert_eq!(cluster.node_count, 0);
        assert_eq!(cluster.total_active, 0);
        assert_eq!(cluster.total_completed, 0);
        assert_eq!(cluster.total_errors, 0);
        assert_eq!(cluster.avg_cluster_latency, 0.0);
    }

    // ── Test 7: Latest snapshot replaces older one for the same node ──────────

    #[test]
    fn test_aggregator_replaces_stale_snapshot() {
        let agg = MetricsAggregator::new();

        agg.record(NodeMetrics::new("node-1", 10, 50, 0, 100.0));
        // Replace with updated snapshot
        agg.record(NodeMetrics::new("node-1", 2, 80, 1, 90.0));

        assert_eq!(
            agg.node_count(),
            1,
            "same node must not create duplicate entries"
        );

        let cluster = agg.aggregate();
        assert_eq!(cluster.total_active, 2, "should use the latest snapshot");
        assert_eq!(cluster.total_completed, 80);
    }

    // ── Test 8: TraceId and SpanId uniqueness ─────────────────────────────────

    #[test]
    fn test_trace_and_span_id_uniqueness() {
        let ids: Vec<TraceId> = (0..20).map(|_| TraceId::new()).collect();
        // All IDs must be unique
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(ids[i], ids[j], "TraceId collision at {i} and {j}");
            }
        }
    }

    // ── Test 9: W3C inject/extract round-trip ────────────────────────────────

    #[test]
    fn test_w3c_inject_extract_roundtrip() {
        let trace_id = "0af7651916cd43dd8448eb211c80319c";
        let span_id = "00f067aa0ba902b7";

        let mut span = DistributedSpan::start("node-a", "test-op");
        span.set_attribute("trace_id", trace_id);
        span.set_attribute("span_id", span_id);
        span.set_attribute("trace_flags", "1");
        span.set_attribute("trace_state", "congo=t61rcWkgMzE");

        let headers = inject_trace_context(&span);

        // Verify traceparent format
        let traceparent = headers
            .get(TRACEPARENT_HEADER)
            .expect("traceparent missing");
        assert_eq!(traceparent, &format!("00-{}-{}-01", trace_id, span_id));

        // Verify tracestate
        let tracestate = headers.get(TRACESTATE_HEADER).expect("tracestate missing");
        assert_eq!(tracestate, "congo=t61rcWkgMzE");

        // Extract and verify
        let ctx = extract_trace_context(&headers).expect("extract must succeed");
        assert_eq!(ctx.trace_id, trace_id);
        assert_eq!(ctx.span_id, span_id);
        assert_eq!(ctx.trace_flags, 0x01);
        assert_eq!(ctx.trace_state.as_deref(), Some("congo=t61rcWkgMzE"));
    }

    // ── Test 10: traceparent format correctness ──────────────────────────────

    #[test]
    fn test_traceparent_format_correctness() {
        let mut span = DistributedSpan::start("node-x", "op");
        span.set_attribute("trace_id", "abcdef01234567890abcdef012345678");
        span.set_attribute("span_id", "0123456789abcdef");
        span.set_attribute("trace_flags", "0");

        let headers = inject_trace_context(&span);
        let tp = headers.get(TRACEPARENT_HEADER).unwrap();

        // Must match: version-traceid-spanid-flags
        let parts: Vec<&str> = tp.split('-').collect();
        assert_eq!(
            parts.len(),
            4,
            "traceparent must have 4 dash-separated parts"
        );
        assert_eq!(parts[0], "00", "version must be 00");
        assert_eq!(parts[1].len(), 32, "trace_id must be 32 hex chars");
        assert_eq!(parts[2].len(), 16, "span_id must be 16 hex chars");
        assert_eq!(parts[3].len(), 2, "flags must be 2 hex chars");
        assert_eq!(parts[3], "00", "flags should be 00");
    }

    // ── Test 11: child span creation with parent linkage ─────────────────────

    #[test]
    fn test_child_span_creation_with_parent_linkage() {
        let parent_ctx = TraceContext {
            trace_id: "aaaabbbbccccdddd1111222233334444".to_string(),
            span_id: "1122334455667788".to_string(),
            parent_span_id: None,
            trace_flags: 0x01,
            trace_state: Some("vendor=opaque".to_string()),
        };

        let propagator = TracePropagator::new();
        let child = propagator.create_child_span(&parent_ctx, "child-operation");

        // Child inherits parent trace_id
        assert_eq!(
            child.attributes.get("trace_id").unwrap(),
            &parent_ctx.trace_id
        );
        // Child records parent span_id as parent_span_id
        assert_eq!(
            child.attributes.get("parent_span_id").unwrap(),
            &parent_ctx.span_id
        );
        // Child has its own span_id, different from parent
        let child_span_id = child.attributes.get("span_id").unwrap();
        assert_ne!(child_span_id, &parent_ctx.span_id);
        assert_eq!(child_span_id.len(), 16, "span_id must be 16 hex chars");
        // Inherits trace_state
        assert_eq!(
            child.attributes.get("trace_state").unwrap(),
            "vendor=opaque"
        );
        // Operation is correct
        assert_eq!(child.operation, "child-operation");
    }

    // ── Test 12: extract from missing headers returns None ───────────────────

    #[test]
    fn test_extract_from_missing_headers_returns_none() {
        // Empty headers
        let empty: HashMap<String, String> = HashMap::new();
        assert!(extract_trace_context(&empty).is_none());

        // Wrong header name
        let mut wrong = HashMap::new();
        wrong.insert("x-trace-id".to_string(), "some-value".to_string());
        assert!(extract_trace_context(&wrong).is_none());

        // Malformed traceparent — wrong number of parts
        let mut bad_parts = HashMap::new();
        bad_parts.insert(TRACEPARENT_HEADER.to_string(), "00-abc".to_string());
        assert!(extract_trace_context(&bad_parts).is_none());

        // Malformed traceparent — wrong version
        let mut bad_version = HashMap::new();
        bad_version.insert(
            TRACEPARENT_HEADER.to_string(),
            "ff-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01".to_string(),
        );
        assert!(extract_trace_context(&bad_version).is_none());

        // Malformed traceparent — wrong trace_id length
        let mut bad_len = HashMap::new();
        bad_len.insert(
            TRACEPARENT_HEADER.to_string(),
            "00-shortid-00f067aa0ba902b7-01".to_string(),
        );
        assert!(extract_trace_context(&bad_len).is_none());

        // Non-hex characters
        let mut bad_hex = HashMap::new();
        bad_hex.insert(
            TRACEPARENT_HEADER.to_string(),
            "00-zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-00f067aa0ba902b7-01".to_string(),
        );
        assert!(extract_trace_context(&bad_hex).is_none());
    }

    // ── Test 13: TracePropagator inject/extract via methods ──────────────────

    #[test]
    fn test_propagator_inject_extract() {
        let propagator = TracePropagator::new();

        let parent = TraceContext::new_root();
        let child = propagator.create_child_span(&parent, "downstream-call");

        let mut headers = HashMap::new();
        propagator.inject(&child, &mut headers);

        assert!(headers.contains_key(TRACEPARENT_HEADER));

        let extracted = propagator.extract(&headers).expect("extract must succeed");
        assert_eq!(extracted.trace_id, parent.trace_id);
        assert_eq!(extracted.span_id, *child.attributes.get("span_id").unwrap());
    }

    // ── Test 14: span correlation ────────────────────────────────────────────

    #[test]
    fn test_span_correlation() {
        let trace_id = "abcdef0123456789abcdef0123456789";

        let mut span = DistributedSpan::start("node-1", "op-a");
        span.set_attribute("trace_id", trace_id);

        // Same trace
        assert!(correlate_spans(&span, trace_id));

        // Different trace
        assert!(!correlate_spans(&span, "00000000000000000000000000000000"));

        // Span without trace_id attribute
        let span_no_trace = DistributedSpan::start("node-2", "op-b");
        assert!(!correlate_spans(&span_no_trace, trace_id));
    }
}
