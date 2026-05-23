# CyberClaw Security Architecture

- Status: Active (M3 implementation in progress)
- Scope: Platform Security
- Owner: CyberClaw Core Team
- Last Updated: 2026-03-21
- Version: 1.0

---

## Executive Summary

CyberClaw's security architecture is built around a **unified SecurityEvent model** that forms the foundation for platform-wide threat detection, policy enforcement, and audit trails. The system implements three layers of protection:

1. **Detection Layer**: SecurityEvent sources detect threats (PromptScanner, RuntimeDetection, PolicyEngine)
2. **Policy Layer**: PolicyEngine enforces governance decisions based on event severity
3. **Audit Layer**: SecurityEventStore maintains immutable audit trails for compliance

This document describes the architectural patterns, data models, and integration points.

---

## 1. Core Models

### 1.1 SecurityEvent

A structured event emitted by runtime detection, policy engines, and security scanners.

```rust
pub struct SecurityEvent {
    pub id: SecurityEventId,                    // Unique event identifier
    pub execution_id: Option<ExecutionId>,      // Associated execution
    pub case_id: Option<CaseId>,                // Associated case
    pub node_id: Option<NodeId>,                // Runtime node
    pub runtime_instance_id: Option<String>,    // Instance identifier
    pub source: SecurityEventSource,            // Where event originated
    pub event_type: SecurityEventType,          // Category of threat
    pub severity: Severity,                     // Risk level (Info, Low, Medium, High, Critical)
    pub summary: String,                        // Human-readable description
    pub details: serde_json::Value,             // Structured event metadata
    pub trace_id: TraceId,                      // Correlation ID for tracing
    pub credential_evidence: Option<SensitiveString>,  // Detected credentials (auto-redacted)
}
```

**Location**: `crates/cyberclaw-core/src/security.rs`

### 1.2 SecurityEventSource

Identifies the component that detected and reported the security event.

```rust
pub enum SecurityEventSource {
    PromptScanner,          // LLM prompt/response analysis
    PackageTrustScanner,    // Dependency verification
    RuntimeDetection,       // Runtime anomaly detection
    PermissionEngine,       // Access control violations
    PolicyEngine,           // Policy enforcement
    PlatformPlugin,         // Extension-based detection
}
```

### 1.3 SecurityEventType

Categorizes the type of security threat or policy violation.

```rust
pub enum SecurityEventType {
    PromptInjectionDetected,    // Detected LLM manipulation
    SkillPoisoningSuspected,    // Malicious capability detected
    RuntimeAnomalyDetected,     // Abnormal runtime behavior
    PermissionViolation,        // Unauthorized operation
    PolicyDenied,               // Policy engine rejected action
    Custom(String),             // Extension-defined events
}
```

### 1.4 Severity

Five-level severity classification for risk prioritization.

```rust
pub enum Severity {
    Info,       // Informational (audit trail only)
    Low,        // Low risk (monitor)
    Medium,     // Medium risk (review required)
    High,       // High risk (escalation required)
    Critical,   // Critical risk (immediate action required)
}
```

---

## 2. Sensitive Data Protection

### 2.1 SensitiveString

Automatic credential redaction to prevent accidental exposure in logs, errors, and serialized output.

**Location**: `crates/cyberclaw-core/src/sensitive.rs`

#### RedactionStrategy

Three strategies for redacting sensitive values:

**Full Strategy**
```rust
RedactionStrategy::Full
// Output: "***REDACTED***"
```

**Partial Strategy** (with configurable prefix/suffix)
```rust
RedactionStrategy::Partial { prefix: 4, suffix: 4 }
// Input:  "sk_live_abcdef1234567890"
// Output: "sk_l****7890"
```

**TypeOnly Strategy** (semantic labels)
```rust
RedactionStrategy::TypeOnly(SensitiveType::ApiKey)
// Output: "<API_KEY>"
```

#### Supported Types

```rust
pub enum SensitiveType {
    Password,   // User passwords and passphrases
    ApiKey,     // Service credentials
    Token,      // OAuth/JWT tokens
    Secret,     // Generic secrets
}
```

### 2.2 Integration Points

SensitiveString automatically redacts in:
- `Debug` trait output (`println!("{:?}", secret)`)
- `Display` trait output (`println!("{}", secret)`)
- Serde JSON serialization (`serde_json::to_string(&secret)`)
- Error messages (never expose value in `Error::display()`)

**Example**:
```rust
use cyberclaw_core::sensitive::SensitiveString;

let api_key = SensitiveString::from_token("sk_live_abc123def456");
println!("{:?}", api_key);   // Outputs: "<TOKEN>"
println!("{}", api_key);     // Outputs: "<TOKEN>"
```

---

## 3. Secrets Management

### 3.1 SecretsManager Trait

Pluggable async interface for secret storage and retrieval.

**Location**: `crates/cyberclaw-core/src/secrets.rs`

```rust
#[async_trait]
pub trait SecretsManager: Send + Sync {
    /// Retrieve plaintext value for key
    async fn get_secret(&self, key: &str) -> Result<String, SecretsError>;

    /// Store value under key (create or overwrite)
    async fn set_secret(&self, key: &str, value: String) -> Result<(), SecretsError>;

    /// Remove the secret under key
    async fn delete_secret(&self, key: &str) -> Result<(), SecretsError>;
}
```

### 3.2 Built-in Implementations

#### InMemorySecretsManager

Development and testing implementation with:
- In-process HashMap-backed storage
- Async audit callback support
- Audit event emission for all operations (get/set/delete)
- Optional TTL support (future)

**Design Goals**:
- Simple interface for rapid development
- Full audit trail for security testing
- Clear upgrade path to production backends

### 3.3 Future Integrations

The trait design supports production backends:

- **HashiCorp Vault**: Enterprise secret management
- **AWS Secrets Manager**: Cloud-native secrets
- **Google Cloud Secret Manager**: Multi-cloud support
- **Azure Key Vault**: Azure ecosystem integration

Implementation is deferred to M4 based on deployment requirements.

---

## 4. Security Event Storage

### 4.1 SecurityEventStore Trait

Persistent storage interface for audit and compliance.

**Location**: `crates/cyberclaw-observability/src/security_event_store.rs`

```rust
#[async_trait]
pub trait SecurityEventStore: Send + Sync {
    /// Record a security event
    async fn record(&self, event: SecurityEvent) -> Result<(), StoreError>;

    /// Query events with filters
    async fn query(&self, filter: &EventFilter) -> Result<Vec<SecurityEvent>, StoreError>;

    /// Verify event immutability (future: with cryptographic proofs)
    async fn verify_chain(&self) -> Result<bool, StoreError>;
}
```

### 4.2 EventFilter

Flexible multi-criteria filtering for audit queries.

```rust
pub struct EventFilter {
    pub execution_id: Option<ExecutionId>,       // Filter by execution
    pub actor: Option<ActorRef>,                 // Filter by actor
    pub event_type: Option<SecurityEventType>,   // Filter by threat type
    pub time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,  // Filter by timespan
}
```

### 4.3 Built-in Implementations

#### InMemorySecurityEventStore

Development and testing implementation:
- In-process Vec-backed storage with Arc<RwLock<>>
- Full EventFilter support
- Suitable for demonstration and integration tests
- Clear upgrade path to persistent backends

### 4.4 Future Integrations

Production-grade backends (M4+):
- **TimescaleDB**: Time-series optimized PostgreSQL
- **ClickHouse**: OLAP analytics engine
- **Elasticsearch**: Full-text searchable audit logs
- **AWS S3 + Athena**: Immutable compliance archives

---

## 5. Event Flow

### 5.1 Detection → Policy → Audit

```
┌─────────────────────────────────────────────────────────────┐
│                    SecurityEvent Flow                        │
└─────────────────────────────────────────────────────────────┘

1. Detection Layer (M3.1 - M3.4)
   ├─ PromptScanner detects injection patterns
   ├─ RuntimeDetection identifies anomalies
   ├─ PolicyEngine evaluates capability risk
   └─ SecurityEvent { id, severity, source, event_type }

2. Policy Layer (M2 - Already implemented)
   ├─ PolicyEngine receives SecurityEvent
   ├─ Maps severity to GovernanceDecision
   │  ├─ Info/Low → Allow
   │  ├─ Medium → ReviewRequired (Human)
   │  ├─ High → ReviewRequired (Approval)
   │  └─ Critical → ReviewRequired (Security)
   └─ execute_governance() enforces decision

3. Audit Layer (M3.5 - M3.7)
   ├─ SecurityEventStore.record(event)
   ├─ Persist to storage (Vec, DB, S3, etc.)
   └─ EventRecorder aggregates all platforms events
```

### 5.2 Integration Points (M3.5+)

**EventRecorder** (crates/cyberclaw-observability/src/events.rs):
- Routes SecurityEvent to storage
- Aggregates events from all sources
- Maintains correlation via execution_id and trace_id

**Orchestrator** (crates/cyberclaw-control-plane/src/orchestrator.rs):
- Emits SecurityEvent on policy decisions
- Passes execution_id to PolicyEngine for correlation
- Triggers review flow on High/Critical severity

---

## 6. Architectural Principles

### 6.1 Separation of Concerns

- **Detection**: Identify threats (PromptScanner, RuntimeDetection)
- **Policy**: Enforce decisions (PolicyEngine, ReviewQueue)
- **Audit**: Record evidence (SecurityEventStore, EventRecorder)

No component bypasses policy; all high-risk operations flow through unified gate.

### 6.2 Fail-Secure

- Deny-by-default for unhandled cases
- Explicit allow decisions only via PolicyEngine
- Security events trigger review, not auto-allow

### 6.3 Audit Trail Integrity

- Unique SecurityEventId + stable ExecutionId ensure traceability
- TraceId enables end-to-end request correlation
- Immutable append-only design (future: cryptographic proofs)

### 6.4 Automatic Credential Masking

- SensitiveString prevents accidental leakage
- All detected credentials stored redacted
- Separate credential_evidence field for forensics

---

## 7. Threat Models

### 7.1 Covered Threats (M3)

| Threat | Detection | Policy | Audit |
|--------|-----------|--------|-------|
| Prompt Injection | PromptScanner | PolicyEngine → Review | SecurityEventStore |
| Skill Poisoning | RuntimeDetection | PolicyEngine → Review | SecurityEventStore |
| Credential Leakage | (auto-redaction) | SensitiveString | credential_evidence field |
| Policy Bypass | N/A | Unified gate (no bypass paths) | Audit trail |
| Unauthorized Review | (future M3.6) | Authorization check | Review logs |

### 7.2 Out of Scope (M4+)

- Container escape (M4.3)
- Process resource exhaustion (M4.2)
- Supply chain attacks (M4+ plugins)
- Cryptographic key rotation (out of scope, uses TLS)

---

## 8. Implementation Status

### 8.1 Completed (M3.1-M3.4)

- [x] SecurityEvent unified model (M3.1)
- [x] SensitiveString redaction (M3.2)
- [x] SecretsManager interface (M3.3)
- [x] SecurityEventStore interface (M3.4)
- [x] InMemory implementations for dev/test

### 8.2 In Progress (M3.5-M3.7)

- [ ] EventRecorder integration (M3.5)
- [ ] Security event triggering review (M3.6)
- [ ] Audit trail end-to-end (M3.7)

### 8.3 Planned (M3.8+)

- [ ] Governance integration tests (M3.8)
- [ ] Production storage backends (M4)
- [ ] Cryptographic event proofs (M4+)

---

## 9. Configuration & Deployment

### 9.1 Development Setup

```rust
// In-memory implementations suitable for all testing
let secrets = Arc::new(InMemorySecretsManager::new());
let event_store = Arc::new(InMemorySecurityEventStore::new());
let policy_engine = Arc::new(DefaultPolicyEngine::new(policies));

let orchestrator = Orchestrator::new(
    policy_engine,
    secrets,
    event_store,
);
```

### 9.2 Production Setup (M4+)

```rust
// Replace with production backends
let secrets = Arc::new(VaultSecretsManager::new(vault_config));
let event_store = Arc::new(ClickHouseEventStore::new(clickhouse_config));
let policy_engine = Arc::new(CustomPolicyEngine::from_file("/etc/policies.yaml"));
```

---

## 10. Testing Strategy

### 10.1 Unit Tests

- SecurityEvent serialization/deserialization
- SensitiveString redaction strategies
- SecretsManager operations (get/set/delete)
- EventFilter query logic

### 10.2 Integration Tests (M3.8)

- End-to-end policy → audit flow
- SecurityEvent persistence and retrieval
- Governance decisions trigger events
- Multi-event correlation via execution_id

### 10.3 Security Tests (M3.7)

- Credential redaction in error messages
- Unauthorized secret access denial
- Policy bypass attempt detection
- Audit trail tampering detection

---

## 11. References

- [BETA_ROADMAP_V1.md](../implementation/roadmap/BETA_ROADMAP_V1.md) - M3 security main chain milestone
- [cyberclaw-core/src/security.rs](../../crates/cyberclaw-core/src/security.rs) - SecurityEvent definitions
- [cyberclaw-core/src/sensitive.rs](../../crates/cyberclaw-core/src/sensitive.rs) - Redaction implementation
- [cyberclaw-core/src/secrets.rs](../../crates/cyberclaw-core/src/secrets.rs) - SecretsManager trait
- [cyberclaw-observability/src/security_event_store.rs](../../crates/cyberclaw-observability/src/security_event_store.rs) - Store interface
- [CHANGELOG.md](../../CHANGELOG.md#m3-security-main-chain-implementation) - M3 implementation details
