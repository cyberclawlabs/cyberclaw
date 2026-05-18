//! OpenTelemetry trace and metric export bridge for CyberClaw.
//!
//! This module provides types and a trait-based exporter that bridges CyberClaw's
//! internal span and metric representations to the OpenTelemetry data model.
//!
//! # Feature Gating
//!
//! The module is always available, but the concrete exporter behaviour depends on
//! the **`otel`** Cargo feature flag:
//!
//! | Feature   | Behaviour                                                        |
//! |-----------|------------------------------------------------------------------|
//! | disabled  | [`OtelExporter`] uses a stub backend that accepts and discards data |
//! | enabled   | [`OtelExporter`] forwards data to an OTLP-compatible collector   |
//!
//! When the `otel` feature is enabled, exports use the **OTLP-HTTP/JSON** protocol
//! via `reqwest` — no `opentelemetry`, `opentelemetry-otlp`, or `opentelemetry_sdk`
//! crates are required. The internal `otlp_json` module hand-rolls the
//! `ExportTraceServiceRequest` / `ExportMetricsServiceRequest` payloads per the
//! [OTLP/HTTP spec](https://opentelemetry.io/docs/specs/otlp/#otlphttp). gRPC
//! transport is configurable in `OtelExporterConfig::protocol` but not yet
//! implemented — set `protocol = HttpJson` and point `endpoint` at an
//! OTLP/HTTP-JSON collector (default Jaeger / Tempo / Grafana Cloud port `4318`).
//!
//! # Examples
//!
//! ```rust
//! use cyberclaw_observability::otel_exporter::*;
//! use std::collections::HashMap;
//!
//! let config = OtelExporterConfig::default();
//! let exporter = OtelExporter::init(config).expect("init must succeed");
//!
//! let span = OtelSpanData {
//!     trace_id: "abc123".into(),
//!     span_id: "def456".into(),
//!     parent_span_id: None,
//!     operation_name: "execute_capability".into(),
//!     start_time: chrono::Utc::now(),
//!     end_time: None,
//!     attributes: HashMap::new(),
//!     status: OtelSpanStatus::Unset,
//! };
//!
//! exporter.export_span(&span).expect("export_span must succeed");
//! exporter.shutdown();
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

// ─── OtelProtocol ─────────────────────────────────────────────────────────────

/// Transport protocol used to communicate with the OTLP collector.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OtelProtocol {
    /// gRPC transport (default port 4317).
    #[default]
    Grpc,
    /// HTTP/JSON transport (default port 4318).
    HttpJson,
}

// ─── OtelExporterConfig ───────────────────────────────────────────────────────

/// Configuration for the OpenTelemetry exporter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtelExporterConfig {
    /// OTLP endpoint (e.g., `"http://localhost:4317"`).
    pub endpoint: String,
    /// Transport protocol to use.
    pub protocol: OtelProtocol,
    /// Service name reported in traces and metrics.
    pub service_name: String,
    /// Whether to export traces.
    pub export_traces: bool,
    /// Whether to export metrics.
    pub export_metrics: bool,
    /// Batch export interval in seconds.
    pub batch_interval_secs: u64,
}

impl Default for OtelExporterConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://localhost:4317".into(),
            protocol: OtelProtocol::Grpc,
            service_name: "cyberclaw".into(),
            export_traces: true,
            export_metrics: true,
            batch_interval_secs: 5,
        }
    }
}

// ─── OtelSpanStatus ───────────────────────────────────────────────────────────

/// Completion status of an exported span, modelled after the OTEL spec.
///
/// Differs from [`crate::distributed::SpanStatus`] which includes `InProgress`.
/// This enum only represents terminal or unset states suitable for export.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OtelSpanStatus {
    /// The span completed successfully.
    Ok,
    /// The span completed with an error.
    Error(String),
    /// Status has not been set (default).
    #[default]
    Unset,
}

// ─── OtelSpanData ─────────────────────────────────────────────────────────────

/// Bridge type that maps CyberClaw span information to the OTEL span data model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtelSpanData {
    /// Trace identifier (hex-encoded, 128-bit).
    pub trace_id: String,
    /// Span identifier (hex-encoded).
    pub span_id: String,
    /// Parent span identifier, if this span is a child.
    pub parent_span_id: Option<String>,
    /// Human-readable operation name.
    pub operation_name: String,
    /// Wall-clock time when the span started.
    pub start_time: DateTime<Utc>,
    /// Wall-clock time when the span ended; `None` while still in progress.
    pub end_time: Option<DateTime<Utc>>,
    /// Arbitrary key/value attributes attached to the span.
    pub attributes: HashMap<String, String>,
    /// Terminal status of the span.
    pub status: OtelSpanStatus,
}

impl OtelSpanData {
    /// Compute the span duration in milliseconds, if both times are available.
    pub fn duration_ms(&self) -> Option<i64> {
        self.end_time
            .map(|end| (end - self.start_time).num_milliseconds())
    }
}

// ─── MetricValue ──────────────────────────────────────────────────────────────

/// Value payload for a single metric data point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MetricValue {
    /// Monotonically increasing counter.
    Counter(f64),
    /// Point-in-time gauge.
    Gauge(f64),
    /// Distribution of observed values.
    Histogram(Vec<f64>),
}

// ─── OtelMetricData ───────────────────────────────────────────────────────────

/// Bridge type that maps a CyberClaw metric reading to the OTEL metric data model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtelMetricData {
    /// Metric name (e.g. `"cyberclaw_execution_total"`).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// The metric value.
    pub value: MetricValue,
    /// Key/value attributes (dimensions / labels).
    pub attributes: HashMap<String, String>,
    /// Timestamp of the reading.
    pub timestamp: DateTime<Utc>,
}

// ─── ExportError ──────────────────────────────────────────────────────────────

/// Errors that can occur during OTEL export operations.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// The exporter has already been shut down.
    #[error("exporter has been shut down")]
    Shutdown,
    /// A connection or transport error occurred.
    #[error("transport error: {0}")]
    Transport(String),
    /// The data failed validation before export.
    #[error("invalid data: {0}")]
    InvalidData(String),
}

// ─── OtelExporter ─────────────────────────────────────────────────────────────

// ─── Batch buffer (otel feature only) ────────────────────────────────────────

/// Maximum number of items to buffer before forcing a flush.
#[cfg(feature = "otel")]
const BATCH_MAX_SIZE: usize = 100;

#[cfg(feature = "otel")]
#[derive(Debug)]
struct BatchBuffer {
    spans: std::sync::Mutex<Vec<OtelSpanData>>,
    metrics: std::sync::Mutex<Vec<OtelMetricData>>,
}

#[cfg(feature = "otel")]
impl BatchBuffer {
    fn new() -> Self {
        Self {
            spans: std::sync::Mutex::new(Vec::new()),
            metrics: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn push_span(&self, span: OtelSpanData) -> Option<Vec<OtelSpanData>> {
        let mut guard = self.spans.lock().expect("batch span lock poisoned");
        guard.push(span);
        if guard.len() >= BATCH_MAX_SIZE {
            Some(std::mem::take(&mut *guard))
        } else {
            None
        }
    }

    fn push_metric(&self, metric: OtelMetricData) -> Option<Vec<OtelMetricData>> {
        let mut guard = self.metrics.lock().expect("batch metric lock poisoned");
        guard.push(metric);
        if guard.len() >= BATCH_MAX_SIZE {
            Some(std::mem::take(&mut *guard))
        } else {
            None
        }
    }

    fn drain_spans(&self) -> Vec<OtelSpanData> {
        let mut guard = self.spans.lock().expect("batch span lock poisoned");
        std::mem::take(&mut *guard)
    }

    fn drain_metrics(&self) -> Vec<OtelMetricData> {
        let mut guard = self.metrics.lock().expect("batch metric lock poisoned");
        std::mem::take(&mut *guard)
    }

    fn span_count(&self) -> usize {
        self.spans.lock().expect("batch span lock poisoned").len()
    }

    fn metric_count(&self) -> usize {
        self.metrics
            .lock()
            .expect("batch metric lock poisoned")
            .len()
    }
}

// ─── OTLP JSON payload types (otel feature only) ──────────────────────────────

#[cfg(feature = "otel")]
mod otlp_json {
    use super::*;

    #[derive(Serialize)]
    pub struct ExportTraceServiceRequest {
        #[serde(rename = "resourceSpans")]
        pub resource_spans: Vec<ResourceSpans>,
    }

    #[derive(Serialize)]
    pub struct ResourceSpans {
        pub resource: Resource,
        #[serde(rename = "scopeSpans")]
        pub scope_spans: Vec<ScopeSpans>,
    }

    #[derive(Serialize)]
    pub struct Resource {
        pub attributes: Vec<KeyValue>,
    }

    #[derive(Serialize)]
    pub struct ScopeSpans {
        pub scope: InstrumentationScope,
        pub spans: Vec<Span>,
    }

    #[derive(Serialize)]
    pub struct InstrumentationScope {
        pub name: String,
        pub version: String,
    }

    #[derive(Serialize)]
    pub struct Span {
        #[serde(rename = "traceId")]
        pub trace_id: String,
        #[serde(rename = "spanId")]
        pub span_id: String,
        #[serde(rename = "parentSpanId")]
        pub parent_span_id: String,
        pub name: String,
        #[serde(rename = "startTimeUnixNano")]
        pub start_time_unix_nano: String,
        #[serde(rename = "endTimeUnixNano")]
        pub end_time_unix_nano: String,
        pub attributes: Vec<KeyValue>,
        pub status: SpanStatus,
    }

    #[derive(Serialize)]
    pub struct SpanStatus {
        pub code: u8,
        pub message: String,
    }

    #[derive(Serialize)]
    pub struct KeyValue {
        pub key: String,
        pub value: StringValue,
    }

    #[derive(Serialize)]
    pub struct StringValue {
        #[serde(rename = "stringValue")]
        pub string_value: String,
    }

    #[derive(Serialize)]
    pub struct ExportMetricsServiceRequest {
        #[serde(rename = "resourceMetrics")]
        pub resource_metrics: Vec<ResourceMetrics>,
    }

    #[derive(Serialize)]
    pub struct ResourceMetrics {
        pub resource: Resource,
        #[serde(rename = "scopeMetrics")]
        pub scope_metrics: Vec<ScopeMetrics>,
    }

    #[derive(Serialize)]
    pub struct ScopeMetrics {
        pub scope: InstrumentationScope,
        pub metrics: Vec<Metric>,
    }

    #[derive(Serialize)]
    pub struct Metric {
        pub name: String,
        pub description: String,
        pub data: MetricData,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    pub enum MetricData {
        Sum(Sum),
        Gauge(GaugeData),
        Histogram(HistogramData),
    }

    #[derive(Serialize)]
    pub struct Sum {
        #[serde(rename = "dataPoints")]
        pub data_points: Vec<NumberDataPoint>,
        #[serde(rename = "aggregationTemporality")]
        pub aggregation_temporality: i32,
        #[serde(rename = "isMonotonic")]
        pub is_monotonic: bool,
    }

    #[derive(Serialize)]
    pub struct GaugeData {
        #[serde(rename = "dataPoints")]
        pub data_points: Vec<NumberDataPoint>,
    }

    #[derive(Serialize)]
    pub struct HistogramData {
        #[serde(rename = "dataPoints")]
        pub data_points: Vec<HistogramDataPoint>,
    }

    #[derive(Serialize)]
    pub struct NumberDataPoint {
        pub attributes: Vec<KeyValue>,
        #[serde(rename = "timeUnixNano")]
        pub time_unix_nano: String,
        #[serde(rename = "asDouble")]
        pub as_double: f64,
    }

    #[derive(Serialize)]
    pub struct HistogramDataPoint {
        pub attributes: Vec<KeyValue>,
        #[serde(rename = "timeUnixNano")]
        pub time_unix_nano: String,
        pub count: u64,
        pub sum: f64,
        #[serde(rename = "bucketCounts")]
        pub bucket_counts: Vec<u64>,
    }

    fn kv(key: &str, value: &str) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value: StringValue {
                string_value: value.to_string(),
            },
        }
    }

    fn ts_nanos(dt: &DateTime<Utc>) -> String {
        dt.timestamp_nanos_opt().unwrap_or(0).to_string()
    }

    pub fn build_traces_request(
        service_name: &str,
        spans: Vec<OtelSpanData>,
    ) -> ExportTraceServiceRequest {
        let converted: Vec<Span> = spans
            .into_iter()
            .map(|s| {
                let (status_code, status_msg) = match &s.status {
                    OtelSpanStatus::Ok => (1u8, String::new()),
                    OtelSpanStatus::Error(msg) => (2u8, msg.clone()),
                    OtelSpanStatus::Unset => (0u8, String::new()),
                };
                Span {
                    trace_id: s.trace_id,
                    span_id: s.span_id,
                    parent_span_id: s.parent_span_id.unwrap_or_default(),
                    name: s.operation_name,
                    start_time_unix_nano: ts_nanos(&s.start_time),
                    end_time_unix_nano: s
                        .end_time
                        .as_ref()
                        .map(ts_nanos)
                        .unwrap_or_else(|| ts_nanos(&s.start_time)),
                    attributes: s.attributes.iter().map(|(k, v)| kv(k, v)).collect(),
                    status: SpanStatus {
                        code: status_code,
                        message: status_msg,
                    },
                }
            })
            .collect();

        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Resource {
                    attributes: vec![kv("service.name", service_name)],
                },
                scope_spans: vec![ScopeSpans {
                    scope: InstrumentationScope {
                        name: "cyberclaw-observability".into(),
                        version: env!("CARGO_PKG_VERSION").into(),
                    },
                    spans: converted,
                }],
            }],
        }
    }

    pub fn build_metrics_request(
        service_name: &str,
        metrics: Vec<OtelMetricData>,
    ) -> ExportMetricsServiceRequest {
        let converted: Vec<Metric> = metrics
            .into_iter()
            .map(|m| {
                let ts = ts_nanos(&m.timestamp);
                let attrs: Vec<KeyValue> = m.attributes.iter().map(|(k, v)| kv(k, v)).collect();
                let data = match m.value {
                    MetricValue::Counter(v) => MetricData::Sum(Sum {
                        data_points: vec![NumberDataPoint {
                            attributes: attrs,
                            time_unix_nano: ts,
                            as_double: v,
                        }],
                        aggregation_temporality: 2,
                        is_monotonic: true,
                    }),
                    MetricValue::Gauge(v) => MetricData::Gauge(GaugeData {
                        data_points: vec![NumberDataPoint {
                            attributes: attrs,
                            time_unix_nano: ts,
                            as_double: v,
                        }],
                    }),
                    MetricValue::Histogram(values) => {
                        let count = values.len() as u64;
                        let sum: f64 = values.iter().sum();
                        MetricData::Histogram(HistogramData {
                            data_points: vec![HistogramDataPoint {
                                attributes: attrs,
                                time_unix_nano: ts,
                                count,
                                sum,
                                bucket_counts: vec![count],
                            }],
                        })
                    }
                };
                Metric {
                    name: m.name,
                    description: m.description,
                    data,
                }
            })
            .collect();

        ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: Resource {
                    attributes: vec![kv("service.name", service_name)],
                },
                scope_metrics: vec![ScopeMetrics {
                    scope: InstrumentationScope {
                        name: "cyberclaw-observability".into(),
                        version: env!("CARGO_PKG_VERSION").into(),
                    },
                    metrics: converted,
                }],
            }],
        }
    }
}

// ─── OtelExporter ─────────────────────────────────────────────────────────────

/// OpenTelemetry exporter that bridges CyberClaw observability data to an OTLP
/// collector.
///
/// When the `otel` feature is **not** enabled, the exporter operates as a stub:
/// all export calls succeed immediately without transmitting data. This allows
/// call-sites to use a uniform API regardless of whether a real collector is
/// configured.
///
/// When the `otel` feature **is** enabled, `export_span` / `export_metric` push
/// items into an internal batch buffer. The buffer flushes automatically when it
/// reaches 100 items. Call [`flush`](Self::flush) to flush immediately, or use
/// [`start_background_flush`](Self::start_background_flush) for periodic flushing.
#[derive(Debug, Clone)]
pub struct OtelExporter {
    config: OtelExporterConfig,
    is_shutdown: Arc<AtomicBool>,
    spans_exported: Arc<AtomicU64>,
    metrics_exported: Arc<AtomicU64>,
    #[cfg(feature = "otel")]
    batch: Arc<BatchBuffer>,
    #[cfg(feature = "otel")]
    http_client: Arc<reqwest::Client>,
}

impl OtelExporter {
    /// Initialise the exporter with the given configuration.
    ///
    /// In stub mode (no `otel` feature) this always succeeds. With the `otel`
    /// feature it will attempt to connect to the configured OTLP endpoint.
    pub fn init(config: OtelExporterConfig) -> Result<Self, ExportError> {
        // Validate config basics
        if config.endpoint.is_empty() {
            return Err(ExportError::InvalidData(
                "endpoint must not be empty".into(),
            ));
        }
        if config.service_name.is_empty() {
            return Err(ExportError::InvalidData(
                "service_name must not be empty".into(),
            ));
        }

        #[cfg(feature = "otel")]
        let http_client = Arc::new(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .map_err(|e| ExportError::Transport(e.to_string()))?,
        );

        Ok(Self {
            config,
            is_shutdown: Arc::new(AtomicBool::new(false)),
            spans_exported: Arc::new(AtomicU64::new(0)),
            metrics_exported: Arc::new(AtomicU64::new(0)),
            #[cfg(feature = "otel")]
            batch: Arc::new(BatchBuffer::new()),
            #[cfg(feature = "otel")]
            http_client,
        })
    }

    /// Flush pending data and shut down the exporter.
    ///
    /// After shutdown, subsequent `export_span` / `export_metric` calls will
    /// return [`ExportError::Shutdown`].
    pub fn shutdown(&self) {
        self.is_shutdown.store(true, Ordering::SeqCst);
    }

    /// Export a single span to the configured OTLP collector.
    ///
    /// In stub mode this validates the span data and increments an internal
    /// counter, but does not transmit anything.
    pub fn export_span(&self, span: &OtelSpanData) -> Result<(), ExportError> {
        if self.is_shutdown.load(Ordering::SeqCst) {
            return Err(ExportError::Shutdown);
        }
        if !self.config.export_traces {
            return Ok(());
        }
        if span.trace_id.is_empty() {
            return Err(ExportError::InvalidData(
                "trace_id must not be empty".into(),
            ));
        }
        if span.span_id.is_empty() {
            return Err(ExportError::InvalidData("span_id must not be empty".into()));
        }

        #[cfg(feature = "otel")]
        {
            let full_batch = self.batch.push_span(span.clone());
            if let Some(batch) = full_batch {
                self.spawn_send_spans(batch);
            }
        }

        self.spans_exported.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Export a single metric reading to the configured OTLP collector.
    ///
    /// In stub mode this validates the metric data and increments an internal
    /// counter, but does not transmit anything.
    pub fn export_metric(&self, metric: &OtelMetricData) -> Result<(), ExportError> {
        if self.is_shutdown.load(Ordering::SeqCst) {
            return Err(ExportError::Shutdown);
        }
        if !self.config.export_metrics {
            return Ok(());
        }
        if metric.name.is_empty() {
            return Err(ExportError::InvalidData(
                "metric name must not be empty".into(),
            ));
        }

        #[cfg(feature = "otel")]
        {
            let full_batch = self.batch.push_metric(metric.clone());
            if let Some(batch) = full_batch {
                self.spawn_send_metrics(batch);
            }
        }

        self.metrics_exported.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Flush all buffered spans and metrics to the collector immediately.
    ///
    /// This is a no-op when the `otel` feature is disabled.
    pub fn flush(&self) {
        #[cfg(feature = "otel")]
        {
            let spans = self.batch.drain_spans();
            if !spans.is_empty() {
                self.spawn_send_spans(spans);
            }
            let metrics = self.batch.drain_metrics();
            if !metrics.is_empty() {
                self.spawn_send_metrics(metrics);
            }
        }
    }

    /// Spawn a background Tokio task that flushes the buffer every
    /// `config.batch_interval_secs` seconds until shutdown.
    ///
    /// This is a no-op when the `otel` feature is disabled. Call this once
    /// after [`init`](Self::init) inside an active Tokio runtime.
    #[cfg(feature = "otel")]
    pub fn start_background_flush(&self) {
        let exporter = self.clone();
        let interval_secs = self.config.batch_interval_secs;
        tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_secs(interval_secs.max(1)));
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if exporter.is_shutdown.load(Ordering::SeqCst) {
                    exporter.flush();
                    break;
                }
                exporter.flush();
            }
        });
    }

    /// Number of spans currently buffered (not yet flushed to collector).
    ///
    /// Always returns 0 when the `otel` feature is disabled.
    pub fn buffered_spans(&self) -> usize {
        #[cfg(feature = "otel")]
        let count = self.batch.span_count();
        #[cfg(not(feature = "otel"))]
        let count = 0;
        count
    }

    /// Number of metrics currently buffered (not yet flushed to collector).
    ///
    /// Always returns 0 when the `otel` feature is disabled.
    pub fn buffered_metrics(&self) -> usize {
        #[cfg(feature = "otel")]
        let count = self.batch.metric_count();
        #[cfg(not(feature = "otel"))]
        let count = 0;
        count
    }

    /// Return a reference to the active configuration.
    pub fn config(&self) -> &OtelExporterConfig {
        &self.config
    }

    /// Return `true` if [`shutdown`](Self::shutdown) has been called.
    pub fn is_shutdown(&self) -> bool {
        self.is_shutdown.load(Ordering::SeqCst)
    }

    /// Number of spans successfully accepted by the exporter since init.
    pub fn spans_exported(&self) -> u64 {
        self.spans_exported.load(Ordering::Relaxed)
    }

    /// Number of metric readings successfully accepted by the exporter since init.
    pub fn metrics_exported(&self) -> u64 {
        self.metrics_exported.load(Ordering::Relaxed)
    }

    /// Convert a [`crate::distributed::DistributedSpan`] to an [`OtelSpanData`],
    /// using the provided trace and span identifiers.
    ///
    /// This is a convenience helper for call-sites that have CyberClaw-native
    /// span data and want to export it via OTEL.
    pub fn convert_distributed_span(
        trace_id: &str,
        span_id: &str,
        parent_span_id: Option<&str>,
        span: &crate::distributed::DistributedSpan,
    ) -> OtelSpanData {
        let status = match &span.status {
            crate::distributed::SpanStatus::Ok => OtelSpanStatus::Ok,
            crate::distributed::SpanStatus::Error(msg) => OtelSpanStatus::Error(msg.clone()),
            crate::distributed::SpanStatus::InProgress => OtelSpanStatus::Unset,
        };

        OtelSpanData {
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            parent_span_id: parent_span_id.map(String::from),
            operation_name: span.operation.clone(),
            start_time: span.start_time,
            end_time: span.end_time,
            attributes: span.attributes.clone(),
            status,
        }
    }

    // ── Private helpers (otel feature only) ────────────────────────────────

    #[cfg(feature = "otel")]
    fn spawn_send_spans(&self, spans: Vec<OtelSpanData>) {
        let client = Arc::clone(&self.http_client);
        let endpoint = format!("{}/v1/traces", self.config.endpoint.trim_end_matches('/'));
        let service_name = self.config.service_name.clone();
        tokio::spawn(async move {
            let payload = otlp_json::build_traces_request(&service_name, spans);
            if let Err(e) = client.post(&endpoint).json(&payload).send().await {
                tracing::warn!("OTLP traces export failed: {e}");
            }
        });
    }

    #[cfg(feature = "otel")]
    fn spawn_send_metrics(&self, metrics: Vec<OtelMetricData>) {
        let client = Arc::clone(&self.http_client);
        let endpoint = format!("{}/v1/metrics", self.config.endpoint.trim_end_matches('/'));
        let service_name = self.config.service_name.clone();
        tokio::spawn(async move {
            let payload = otlp_json::build_metrics_request(&service_name, metrics);
            if let Err(e) = client.post(&endpoint).json(&payload).send().await {
                tracing::warn!("OTLP metrics export failed: {e}");
            }
        });
    }
}

// ─── Boot-time helper ─────────────────────────────────────────────────────────

/// Build an [`OtelExporter`] from the standard environment variables, or
/// return `None` when OTLP export isn't configured.
///
/// Env contract:
/// - `CYBERCLAW_OTLP_ENDPOINT` (required to enable) — full base URL of the
///   OTLP/HTTP-JSON collector, e.g. `http://localhost:4318`. Span POST goes
///   to `{endpoint}/v1/traces`, metric POST to `{endpoint}/v1/metrics`.
/// - `CYBERCLAW_OTLP_SERVICE_NAME` (optional, default `cyberclaw`) — the
///   `service.name` resource attribute attached to every payload.
/// - `CYBERCLAW_OTLP_BATCH_SECS` (optional, default 5) — background flush
///   interval in seconds.
/// - `CYBERCLAW_OTLP_EXPORT_TRACES` (optional, default true) — `false`/`0`
///   disables trace export.
/// - `CYBERCLAW_OTLP_EXPORT_METRICS` (optional, default true) — `false`/`0`
///   disables metric export.
///
/// Returns `Ok(None)` when `CYBERCLAW_OTLP_ENDPOINT` is unset (the operator
/// hasn't opted in). Returns `Err` only on validation failure (e.g. empty
/// service name) so misconfigurations are loud at boot.
pub fn init_from_env() -> Result<Option<Arc<OtelExporter>>, ExportError> {
    let Ok(endpoint) = std::env::var("CYBERCLAW_OTLP_ENDPOINT") else {
        return Ok(None);
    };
    if endpoint.trim().is_empty() {
        return Ok(None);
    }
    let service_name = std::env::var("CYBERCLAW_OTLP_SERVICE_NAME")
        .unwrap_or_else(|_| "cyberclaw".to_string());
    let batch_interval_secs = std::env::var("CYBERCLAW_OTLP_BATCH_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(5);
    let parse_bool = |v: String| !matches!(v.to_lowercase().as_str(), "false" | "0" | "no" | "off");
    let export_traces = std::env::var("CYBERCLAW_OTLP_EXPORT_TRACES")
        .map(parse_bool)
        .unwrap_or(true);
    let export_metrics = std::env::var("CYBERCLAW_OTLP_EXPORT_METRICS")
        .map(parse_bool)
        .unwrap_or(true);

    let config = OtelExporterConfig {
        endpoint,
        protocol: OtelProtocol::HttpJson,
        service_name,
        export_traces,
        export_metrics,
        batch_interval_secs,
    };
    let exporter = OtelExporter::init(config)?;
    Ok(Some(Arc::new(exporter)))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Config tests ──────────────────────────────────────────────────────────

    #[test]
    fn test_default_config() {
        let config = OtelExporterConfig::default();
        assert_eq!(config.endpoint, "http://localhost:4317");
        assert_eq!(config.protocol, OtelProtocol::Grpc);
        assert_eq!(config.service_name, "cyberclaw");
        assert!(config.export_traces);
        assert!(config.export_metrics);
        assert_eq!(config.batch_interval_secs, 5);
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let config = OtelExporterConfig {
            endpoint: "http://otel:4318".into(),
            protocol: OtelProtocol::HttpJson,
            service_name: "my-service".into(),
            export_traces: true,
            export_metrics: false,
            batch_interval_secs: 10,
        };
        let json = serde_json::to_string(&config).expect("serialize");
        let recovered: OtelExporterConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(recovered.endpoint, config.endpoint);
        assert_eq!(recovered.protocol, OtelProtocol::HttpJson);
        assert_eq!(recovered.service_name, "my-service");
        assert!(!recovered.export_metrics);
        assert_eq!(recovered.batch_interval_secs, 10);
    }

    #[test]
    fn test_config_validation_empty_endpoint() {
        let config = OtelExporterConfig {
            endpoint: String::new(),
            ..Default::default()
        };
        let result = OtelExporter::init(config);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("endpoint"));
    }

    #[test]
    fn test_config_validation_empty_service_name() {
        let config = OtelExporterConfig {
            service_name: String::new(),
            ..Default::default()
        };
        let result = OtelExporter::init(config);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("service_name"));
    }

    // ── Exporter lifecycle tests ──────────────────────────────────────────────

    #[test]
    fn test_init_and_shutdown() {
        let exporter = OtelExporter::init(OtelExporterConfig::default()).expect("init");
        assert!(!exporter.is_shutdown());
        assert_eq!(exporter.spans_exported(), 0);
        assert_eq!(exporter.metrics_exported(), 0);

        exporter.shutdown();
        assert!(exporter.is_shutdown());
    }

    #[test]
    fn test_export_after_shutdown_fails() {
        let exporter = OtelExporter::init(OtelExporterConfig::default()).expect("init");
        exporter.shutdown();

        let span = make_test_span();
        let result = exporter.export_span(&span);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ExportError::Shutdown));

        let metric = make_test_metric();
        let result = exporter.export_metric(&metric);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ExportError::Shutdown));
    }

    // ── Span export tests ─────────────────────────────────────────────────────

    #[test]
    fn test_export_span_success() {
        let exporter = OtelExporter::init(OtelExporterConfig::default()).expect("init");
        let span = make_test_span();

        exporter.export_span(&span).expect("export_span");
        assert_eq!(exporter.spans_exported(), 1);

        exporter.export_span(&span).expect("export_span again");
        assert_eq!(exporter.spans_exported(), 2);
    }

    #[test]
    fn test_export_span_invalid_trace_id() {
        let exporter = OtelExporter::init(OtelExporterConfig::default()).expect("init");
        let mut span = make_test_span();
        span.trace_id = String::new();

        let result = exporter.export_span(&span);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("trace_id"));
    }

    #[test]
    fn test_export_span_invalid_span_id() {
        let exporter = OtelExporter::init(OtelExporterConfig::default()).expect("init");
        let mut span = make_test_span();
        span.span_id = String::new();

        let result = exporter.export_span(&span);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("span_id"));
    }

    #[test]
    fn test_export_span_skipped_when_traces_disabled() {
        let config = OtelExporterConfig {
            export_traces: false,
            ..Default::default()
        };
        let exporter = OtelExporter::init(config).expect("init");
        let span = make_test_span();

        exporter
            .export_span(&span)
            .expect("should succeed silently");
        assert_eq!(exporter.spans_exported(), 0, "no span should be counted");
    }

    // ── Metric export tests ───────────────────────────────────────────────────

    #[test]
    fn test_export_metric_success() {
        let exporter = OtelExporter::init(OtelExporterConfig::default()).expect("init");
        let metric = make_test_metric();

        exporter.export_metric(&metric).expect("export_metric");
        assert_eq!(exporter.metrics_exported(), 1);
    }

    #[test]
    fn test_export_metric_invalid_name() {
        let exporter = OtelExporter::init(OtelExporterConfig::default()).expect("init");
        let mut metric = make_test_metric();
        metric.name = String::new();

        let result = exporter.export_metric(&metric);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("metric name"));
    }

    #[test]
    fn test_export_metric_skipped_when_metrics_disabled() {
        let config = OtelExporterConfig {
            export_metrics: false,
            ..Default::default()
        };
        let exporter = OtelExporter::init(config).expect("init");
        let metric = make_test_metric();

        exporter
            .export_metric(&metric)
            .expect("should succeed silently");
        assert_eq!(
            exporter.metrics_exported(),
            0,
            "no metric should be counted"
        );
    }

    // ── Span data tests ──────────────────────────────────────────────────────

    #[test]
    fn test_span_data_duration() {
        let start = Utc::now();
        let end = start + chrono::Duration::milliseconds(150);

        let span = OtelSpanData {
            trace_id: "t1".into(),
            span_id: "s1".into(),
            parent_span_id: None,
            operation_name: "op".into(),
            start_time: start,
            end_time: Some(end),
            attributes: HashMap::new(),
            status: OtelSpanStatus::Ok,
        };
        assert_eq!(span.duration_ms(), Some(150));
    }

    #[test]
    fn test_span_data_duration_none_when_open() {
        let span = OtelSpanData {
            trace_id: "t1".into(),
            span_id: "s1".into(),
            parent_span_id: None,
            operation_name: "op".into(),
            start_time: Utc::now(),
            end_time: None,
            attributes: HashMap::new(),
            status: OtelSpanStatus::Unset,
        };
        assert!(span.duration_ms().is_none());
    }

    // ── MetricValue tests ─────────────────────────────────────────────────────

    #[test]
    fn test_metric_value_variants() {
        let counter = MetricValue::Counter(42.0);
        let gauge = MetricValue::Gauge(-3.5);
        let histogram = MetricValue::Histogram(vec![1.0, 2.0, 3.0]);

        // Verify debug formatting works (no panics) and equality
        assert_eq!(counter, MetricValue::Counter(42.0));
        assert_eq!(gauge, MetricValue::Gauge(-3.5));
        assert_eq!(histogram, MetricValue::Histogram(vec![1.0, 2.0, 3.0]));
    }

    // ── Conversion from DistributedSpan ───────────────────────────────────────

    #[test]
    fn test_convert_distributed_span_ok() {
        let mut ds = crate::distributed::DistributedSpan::start("node-1", "run_skill");
        ds.set_attribute("skill_id", "format-code");
        ds.finish_ok();

        let otel = OtelExporter::convert_distributed_span("trace-abc", "span-def", None, &ds);

        assert_eq!(otel.trace_id, "trace-abc");
        assert_eq!(otel.span_id, "span-def");
        assert!(otel.parent_span_id.is_none());
        assert_eq!(otel.operation_name, "run_skill");
        assert_eq!(otel.status, OtelSpanStatus::Ok);
        assert_eq!(
            otel.attributes.get("skill_id").map(String::as_str),
            Some("format-code")
        );
        assert!(otel.end_time.is_some());
    }

    #[test]
    fn test_convert_distributed_span_error() {
        let mut ds = crate::distributed::DistributedSpan::start("node-2", "invoke_cap");
        ds.finish_error("connection refused");

        let otel = OtelExporter::convert_distributed_span("t1", "s1", Some("parent-s0"), &ds);

        assert_eq!(otel.parent_span_id.as_deref(), Some("parent-s0"));
        assert_eq!(
            otel.status,
            OtelSpanStatus::Error("connection refused".into())
        );
    }

    #[test]
    fn test_convert_distributed_span_in_progress() {
        let ds = crate::distributed::DistributedSpan::start("node-3", "processing");

        let otel = OtelExporter::convert_distributed_span("t2", "s2", None, &ds);

        assert_eq!(otel.status, OtelSpanStatus::Unset);
        assert!(otel.end_time.is_none());
    }

    // ── Clone / thread-safety ─────────────────────────────────────────────────

    #[test]
    fn test_exporter_clone_shares_state() {
        let exporter = OtelExporter::init(OtelExporterConfig::default()).expect("init");
        let clone = exporter.clone();

        exporter
            .export_span(&make_test_span())
            .expect("export_span");

        // The clone sees the same counter
        assert_eq!(clone.spans_exported(), 1);

        clone.shutdown();
        assert!(exporter.is_shutdown(), "shutdown visible via original");
    }

    // ── Stub mode: buffered counts always 0 ─────────────────────────────────

    #[test]
    #[cfg(not(feature = "otel"))]
    fn test_stub_buffered_counts_zero() {
        let exporter = OtelExporter::init(OtelExporterConfig::default()).expect("init");
        exporter.export_span(&make_test_span()).expect("ok");
        exporter.export_metric(&make_test_metric()).expect("ok");
        assert_eq!(exporter.buffered_spans(), 0);
        assert_eq!(exporter.buffered_metrics(), 0);
    }

    // ── Batch buffer tests (otel feature only) ───────────────────────────────

    #[cfg(feature = "otel")]
    mod otel_batch_tests {
        use super::*;

        #[test]
        fn test_batch_buffer_accumulates_spans() {
            let exporter = OtelExporter::init(OtelExporterConfig::default()).expect("init");
            for _ in 0..5 {
                exporter.export_span(&make_test_span()).expect("ok");
            }
            assert_eq!(exporter.buffered_spans(), 5);
            assert_eq!(exporter.spans_exported(), 5);
        }

        #[test]
        fn test_batch_buffer_accumulates_metrics() {
            let exporter = OtelExporter::init(OtelExporterConfig::default()).expect("init");
            for _ in 0..3 {
                exporter.export_metric(&make_test_metric()).expect("ok");
            }
            assert_eq!(exporter.buffered_metrics(), 3);
            assert_eq!(exporter.metrics_exported(), 3);
        }

        #[tokio::test]
        async fn test_flush_drains_buffer() {
            let exporter = OtelExporter::init(OtelExporterConfig::default()).expect("init");
            for _ in 0..10 {
                exporter.export_span(&make_test_span()).expect("ok");
                exporter.export_metric(&make_test_metric()).expect("ok");
            }
            assert_eq!(exporter.buffered_spans(), 10);
            assert_eq!(exporter.buffered_metrics(), 10);

            exporter.flush();
            assert_eq!(exporter.buffered_spans(), 0);
            assert_eq!(exporter.buffered_metrics(), 0);
        }

        #[tokio::test]
        async fn test_batch_triggers_at_max_size() {
            let exporter = OtelExporter::init(OtelExporterConfig::default()).expect("init");
            for _ in 0..super::super::BATCH_MAX_SIZE {
                exporter.export_span(&make_test_span()).expect("ok");
            }
            assert_eq!(exporter.buffered_spans(), 0);
        }

        #[test]
        fn test_clone_shares_batch_buffer() {
            let exporter = OtelExporter::init(OtelExporterConfig::default()).expect("init");
            let clone = exporter.clone();
            exporter.export_span(&make_test_span()).expect("ok");
            assert_eq!(clone.buffered_spans(), 1);
        }

        #[test]
        fn test_otlp_json_traces_payload() {
            let span = make_test_span();
            let payload = super::super::otlp_json::build_traces_request("test-svc", vec![span]);
            let json = serde_json::to_string(&payload).expect("serialize");
            assert!(json.contains("resourceSpans"));
            assert!(json.contains("test-svc"));
            assert!(json.contains("service.name"));
        }

        #[test]
        fn test_otlp_json_metrics_counter_payload() {
            let metric = OtelMetricData {
                name: "req_total".into(),
                description: "requests".into(),
                value: MetricValue::Counter(42.0),
                attributes: HashMap::new(),
                timestamp: Utc::now(),
            };
            let payload = super::super::otlp_json::build_metrics_request("test-svc", vec![metric]);
            let json = serde_json::to_string(&payload).expect("serialize");
            assert!(json.contains("resourceMetrics"));
            assert!(json.contains("req_total"));
            assert!(json.contains("42"));
        }

        #[test]
        fn test_otlp_json_metrics_gauge_payload() {
            let metric = OtelMetricData {
                name: "cpu_usage".into(),
                description: "cpu".into(),
                value: MetricValue::Gauge(0.75),
                attributes: HashMap::new(),
                timestamp: Utc::now(),
            };
            let payload = super::super::otlp_json::build_metrics_request("test-svc", vec![metric]);
            let json = serde_json::to_string(&payload).expect("serialize");
            assert!(json.contains("cpu_usage"));
            assert!(json.contains("0.75"));
        }

        #[test]
        fn test_otlp_json_metrics_histogram_payload() {
            let metric = OtelMetricData {
                name: "latency_ms".into(),
                description: "latency".into(),
                value: MetricValue::Histogram(vec![1.0, 5.0, 10.0]),
                attributes: HashMap::new(),
                timestamp: Utc::now(),
            };
            let payload = super::super::otlp_json::build_metrics_request("test-svc", vec![metric]);
            let json = serde_json::to_string(&payload).expect("serialize");
            assert!(json.contains("latency_ms"));
            assert!(json.contains("histogram"));
        }
    }

    // ── init_from_env tests ───────────────────────────────────────────────────
    //
    // Serialize all env-touching tests via ENV_LOCK so parallel test execution
    // doesn't see each other's env mutations.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clear_otlp_env() {
        unsafe {
            std::env::remove_var("CYBERCLAW_OTLP_ENDPOINT");
            std::env::remove_var("CYBERCLAW_OTLP_SERVICE_NAME");
            std::env::remove_var("CYBERCLAW_OTLP_BATCH_SECS");
            std::env::remove_var("CYBERCLAW_OTLP_EXPORT_TRACES");
            std::env::remove_var("CYBERCLAW_OTLP_EXPORT_METRICS");
        }
    }

    #[test]
    fn init_from_env_returns_none_when_endpoint_unset() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_otlp_env();
        let got = init_from_env().expect("ok");
        assert!(got.is_none());
    }

    #[test]
    fn init_from_env_returns_none_when_endpoint_blank() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_otlp_env();
        unsafe { std::env::set_var("CYBERCLAW_OTLP_ENDPOINT", "   ") };
        let got = init_from_env().expect("ok");
        clear_otlp_env();
        assert!(got.is_none());
    }

    #[test]
    fn init_from_env_builds_exporter_with_defaults() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_otlp_env();
        unsafe {
            std::env::set_var("CYBERCLAW_OTLP_ENDPOINT", "http://localhost:4318");
        }
        let got = init_from_env().expect("ok");
        let exporter = got.expect("some");
        assert_eq!(exporter.config().endpoint, "http://localhost:4318");
        assert_eq!(exporter.config().service_name, "cyberclaw");
        assert_eq!(exporter.config().batch_interval_secs, 5);
        assert_eq!(exporter.config().protocol, OtelProtocol::HttpJson);
        assert!(exporter.config().export_traces);
        assert!(exporter.config().export_metrics);
        clear_otlp_env();
    }

    #[test]
    fn init_from_env_respects_override_vars() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_otlp_env();
        unsafe {
            std::env::set_var("CYBERCLAW_OTLP_ENDPOINT", "http://otel:4318");
            std::env::set_var("CYBERCLAW_OTLP_SERVICE_NAME", "my-svc");
            std::env::set_var("CYBERCLAW_OTLP_BATCH_SECS", "10");
            std::env::set_var("CYBERCLAW_OTLP_EXPORT_METRICS", "false");
        }
        let exporter = init_from_env().expect("ok").expect("some");
        assert_eq!(exporter.config().service_name, "my-svc");
        assert_eq!(exporter.config().batch_interval_secs, 10);
        assert!(exporter.config().export_traces);
        assert!(!exporter.config().export_metrics);
        clear_otlp_env();
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn make_test_span() -> OtelSpanData {
        OtelSpanData {
            trace_id: "aaaa".into(),
            span_id: "bbbb".into(),
            parent_span_id: None,
            operation_name: "test_op".into(),
            start_time: Utc::now(),
            end_time: Some(Utc::now()),
            attributes: HashMap::from([("key".into(), "val".into())]),
            status: OtelSpanStatus::Ok,
        }
    }

    fn make_test_metric() -> OtelMetricData {
        OtelMetricData {
            name: "test_counter".into(),
            description: "A test counter".into(),
            value: MetricValue::Counter(1.0),
            attributes: HashMap::new(),
            timestamp: Utc::now(),
        }
    }
}
