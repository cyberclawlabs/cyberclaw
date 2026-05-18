use crate::identity::ActorRef;
use crate::ids::{CaseId, ExecutionId, NodeId, SecurityEventId, TraceId};
use crate::sensitive::SensitiveString;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityEventSource {
    PromptScanner,
    PackageTrustScanner,
    RuntimeDetection,
    PermissionEngine,
    PolicyEngine,
    PlatformPlugin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityEventType {
    PromptInjectionDetected,
    SkillPoisoningSuspected,
    RuntimeAnomalyDetected,
    PermissionViolation,
    PolicyDenied,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

/// A structured security event emitted by runtime detection and policy engines.
///
/// Any credential or token captured during detection is stored as a
/// [`SensitiveString`] so that it is automatically redacted in debug output
/// and serialised log records.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityEvent {
    pub id: SecurityEventId,
    pub actor: Option<ActorRef>,
    pub timestamp: DateTime<Utc>,
    pub execution_id: Option<ExecutionId>,
    pub case_id: Option<CaseId>,
    pub node_id: Option<NodeId>,
    pub runtime_instance_id: Option<String>,
    pub source: SecurityEventSource,
    pub event_type: SecurityEventType,
    pub severity: Severity,
    pub summary: String,
    pub details: serde_json::Value,
    pub trace_id: TraceId,
    /// Optional credential evidence captured during detection.  Always
    /// redacted in debug / log output.
    pub credential_evidence: Option<SensitiveString>,
}
