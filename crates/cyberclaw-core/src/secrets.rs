//! Secrets management for the CyberClaw platform.
//!
//! Provides the [`SecretsManager`] trait and an in-memory mock implementation
//! ([`InMemorySecretsManager`]) suitable for testing and development environments.
//!
//! # Security Model
//!
//! All secret access operations are recorded as [`SecurityEvent`]s via an optional
//! audit callback. Secret values are never included in audit logs or error messages.
//!
//! # Example
//!
//! ```rust
//! use cyberclaw_core::secrets::{InMemorySecretsManager, SecretsManager};
//!
//! # tokio_test::block_on(async {
//! let manager = InMemorySecretsManager::new();
//! manager.set_secret("api-key", "s3cr3t".to_string()).await.unwrap();
//! let value = manager.get_secret("api-key").await.unwrap();
//! assert_eq!(value, "s3cr3t");
//! # });
//! ```

use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;

use crate::identity::Identity;
use crate::ids::{SecurityEventId, TraceId};
use crate::security::{SecurityEvent, SecurityEventSource, SecurityEventType, Severity};

// ── Error type ────────────────────────────────────────────────────────────────

/// Errors that may be returned by [`SecretsManager`] operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SecretsError {
    /// The requested key does not exist in the store.
    #[error("secret not found: {key}")]
    NotFound { key: String },

    /// The caller is not authorised to perform this operation.
    #[error("access denied for actor '{actor}' on key '{key}'")]
    AccessDenied { actor: String, key: String },

    /// A backend I/O or storage error occurred (reserved for future backends).
    #[error("secrets backend error: {0}")]
    Backend(String),
}

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Async interface for secret key/value storage.
///
/// Implementations must be `Send + Sync` so they can be shared across async tasks.
/// All operations are audited via the platform's security event mechanism when an
/// audit sink has been configured.
///
/// # Implementing a custom backend
///
/// Replace [`InMemorySecretsManager`] with a struct that wraps a real secrets
/// backend (e.g. HashiCorp Vault, AWS Secrets Manager) by implementing this trait.
#[async_trait]
pub trait SecretsManager: Send + Sync {
    /// Retrieve the plaintext value for `key`.
    ///
    /// # Errors
    ///
    /// Returns [`SecretsError::NotFound`] when `key` is absent.
    async fn get_secret(&self, key: &str) -> Result<String, SecretsError>;

    /// Store `value` under `key`, creating or overwriting any existing entry.
    ///
    /// # Errors
    ///
    /// Returns [`SecretsError::AccessDenied`] when the caller lacks write permission.
    async fn set_secret(&self, key: &str, value: String) -> Result<(), SecretsError>;

    /// Remove the secret stored under `key`.
    ///
    /// # Errors
    ///
    /// Returns [`SecretsError::NotFound`] when `key` is absent.
    async fn delete_secret(&self, key: &str) -> Result<(), SecretsError>;

    /// Return a sorted list of all stored key names.
    ///
    /// Secret *values* are never included in the returned list.
    async fn list_keys(&self) -> Result<Vec<String>, SecretsError>;
}

// ── Audit sink type alias ─────────────────────────────────────────────────────

/// Callback invoked after each secrets operation to record a [`SecurityEvent`].
///
/// The callback receives an owned event so it can be forwarded to any sink
/// (in-memory queue, tracing span, metrics counter, etc.) without additional
/// allocation.
pub type AuditSink = Arc<dyn Fn(SecurityEvent) + Send + Sync + 'static>;

// ── In-memory implementation ──────────────────────────────────────────────────

/// Thread-safe, in-memory secrets store intended for testing and local development.
///
/// All reads and writes are protected by a [`tokio::sync::RwLock`] so multiple
/// async tasks can safely share a single instance via [`Arc`].
///
/// When an [`AuditSink`] is registered (see [`InMemorySecretsManager::with_audit`]),
/// every `get`, `set`, and `delete` call emits a [`SecurityEvent`].  Secret
/// *values* are **never** included in emitted events.
///
/// # Example
///
/// ```rust
/// use cyberclaw_core::secrets::{InMemorySecretsManager, SecretsManager};
/// use std::sync::{Arc, Mutex};
///
/// # tokio_test::block_on(async {
/// let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
/// let log_clone = Arc::clone(&log);
///
/// let manager = InMemorySecretsManager::new()
///     .with_audit(Arc::new(move |event| {
///         log_clone.lock().unwrap().push(event.summary.clone());
///     }));
///
/// manager.set_secret("token", "abc123".to_string()).await.unwrap();
/// assert!(!log.lock().unwrap().is_empty());
/// # });
/// ```
pub struct InMemorySecretsManager {
    store: Arc<RwLock<HashMap<String, String>>>,
    audit: Option<AuditSink>,
}

impl InMemorySecretsManager {
    /// Create a new, empty secrets store without an audit sink.
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
            audit: None,
        }
    }

    /// Attach an audit sink.  Returns `self` for builder-style chaining.
    ///
    /// # Example
    ///
    /// ```rust
    /// use cyberclaw_core::secrets::InMemorySecretsManager;
    /// use std::sync::Arc;
    ///
    /// let manager = InMemorySecretsManager::new()
    ///     .with_audit(Arc::new(|event| {
    ///         // forward to tracing or metrics
    ///         let _ = event;
    ///     }));
    /// ```
    #[must_use]
    pub fn with_audit(mut self, sink: AuditSink) -> Self {
        self.audit = Some(sink);
        self
    }

    /// Emit an audit event if a sink has been registered.
    ///
    /// The secret *value* is intentionally **not** passed to this method; the
    /// `summary` and `details` fields must never contain plaintext secret material.
    fn emit_audit(&self, operation: &str, key: &str, outcome: &str) {
        if let Some(sink) = &self.audit {
            // SECURITY FIX: Secrets operations are system-level, use System identity
            let actor = Identity::System.to_actor_ref(None);
            let event = SecurityEvent {
                id: SecurityEventId::new(),
                actor,
                timestamp: Utc::now(),
                execution_id: None,
                case_id: None,
                node_id: None,
                runtime_instance_id: None,
                source: SecurityEventSource::PermissionEngine,
                event_type: SecurityEventType::Custom(format!("secrets.{operation}")),
                severity: Severity::Info,
                // SECURITY: key name is logged, but value is never included.
                summary: format!("secrets.{operation}: key='{key}' outcome={outcome}"),
                details: serde_json::json!({
                    "operation": operation,
                    "key": key,
                    "outcome": outcome,
                }),
                trace_id: TraceId::new(),
                credential_evidence: None,
            };
            sink(event);
        }
    }
}

impl Default for InMemorySecretsManager {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SecretsManager for InMemorySecretsManager {
    async fn get_secret(&self, key: &str) -> Result<String, SecretsError> {
        let result = {
            let store = self.store.read().await;
            store.get(key).cloned()
        };
        match result {
            Some(value) => {
                self.emit_audit("get", key, "ok");
                Ok(value)
            }
            None => {
                self.emit_audit("get", key, "not_found");
                Err(SecretsError::NotFound {
                    key: key.to_string(),
                })
            }
        }
    }

    async fn set_secret(&self, key: &str, value: String) -> Result<(), SecretsError> {
        let mut store = self.store.write().await;
        store.insert(key.to_string(), value);
        drop(store); // release write lock before emitting audit
                     // SECURITY: value is NOT passed to emit_audit.
        self.emit_audit("set", key, "ok");
        Ok(())
    }

    async fn delete_secret(&self, key: &str) -> Result<(), SecretsError> {
        let mut store = self.store.write().await;
        match store.remove(key) {
            Some(_) => {
                drop(store);
                self.emit_audit("delete", key, "ok");
                Ok(())
            }
            None => {
                drop(store);
                self.emit_audit("delete", key, "not_found");
                Err(SecretsError::NotFound {
                    key: key.to_string(),
                })
            }
        }
    }

    async fn list_keys(&self) -> Result<Vec<String>, SecretsError> {
        let store = self.store.read().await;
        let mut keys: Vec<String> = store.keys().cloned().collect();
        drop(store);
        keys.sort();
        Ok(keys)
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Helper: create a manager whose audit events are collected into a Vec.
    fn manager_with_log() -> (InMemorySecretsManager, Arc<Mutex<Vec<SecurityEvent>>>) {
        let log: Arc<Mutex<Vec<SecurityEvent>>> = Arc::new(Mutex::new(vec![]));
        let log_clone = Arc::clone(&log);
        let manager = InMemorySecretsManager::new().with_audit(Arc::new(move |evt| {
            log_clone.lock().unwrap().push(evt);
        }));
        (manager, log)
    }

    // ── set / get ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_set_and_get_secret() {
        let m = InMemorySecretsManager::new();
        m.set_secret("db-password", "hunter2".to_string())
            .await
            .unwrap();
        let val = m.get_secret("db-password").await.unwrap();
        assert_eq!(val, "hunter2");
    }

    #[tokio::test]
    async fn test_set_overwrites_existing() {
        let m = InMemorySecretsManager::new();
        m.set_secret("key", "v1".to_string()).await.unwrap();
        m.set_secret("key", "v2".to_string()).await.unwrap();
        let val = m.get_secret("key").await.unwrap();
        assert_eq!(val, "v2");
    }

    #[tokio::test]
    async fn test_get_missing_key_returns_not_found() {
        let m = InMemorySecretsManager::new();
        let err = m.get_secret("missing").await.unwrap_err();
        assert!(matches!(err, SecretsError::NotFound { key } if key == "missing"));
    }

    // ── delete ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_delete_existing_key() {
        let m = InMemorySecretsManager::new();
        m.set_secret("token", "abc".to_string()).await.unwrap();
        m.delete_secret("token").await.unwrap();
        assert!(matches!(
            m.get_secret("token").await.unwrap_err(),
            SecretsError::NotFound { .. }
        ));
    }

    #[tokio::test]
    async fn test_delete_missing_key_returns_not_found() {
        let m = InMemorySecretsManager::new();
        let err = m.delete_secret("ghost").await.unwrap_err();
        assert!(matches!(err, SecretsError::NotFound { key } if key == "ghost"));
    }

    // ── list_keys ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_list_keys_empty() {
        let m = InMemorySecretsManager::new();
        let keys = m.list_keys().await.unwrap();
        assert!(keys.is_empty());
    }

    #[tokio::test]
    async fn test_list_keys_sorted() {
        let m = InMemorySecretsManager::new();
        m.set_secret("zebra", "z".to_string()).await.unwrap();
        m.set_secret("alpha", "a".to_string()).await.unwrap();
        m.set_secret("middle", "m".to_string()).await.unwrap();
        let keys = m.list_keys().await.unwrap();
        assert_eq!(keys, vec!["alpha", "middle", "zebra"]);
    }

    #[tokio::test]
    async fn test_list_keys_does_not_include_values() {
        let m = InMemorySecretsManager::new();
        m.set_secret("my-key", "super-secret-value".to_string())
            .await
            .unwrap();
        let keys = m.list_keys().await.unwrap();
        // Value must not appear in any key name
        for k in &keys {
            assert!(!k.contains("super-secret-value"));
        }
    }

    // ── audit ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_audit_set_emits_event() {
        let (m, log) = manager_with_log();
        m.set_secret("api-key", "s3cr3t".to_string()).await.unwrap();
        let events = log.lock().unwrap();
        assert_eq!(events.len(), 1);
        let ev = &events[0];
        assert!(ev.summary.contains("secrets.set"));
        assert!(ev.summary.contains("api-key"));
        // SECURITY: the actual value must NOT appear in the summary or details
        assert!(!ev.summary.contains("s3cr3t"));
        let details_str = ev.details.to_string();
        assert!(!details_str.contains("s3cr3t"));
    }

    #[tokio::test]
    async fn test_audit_get_emits_event_ok() {
        let (m, log) = manager_with_log();
        m.set_secret("k", "v".to_string()).await.unwrap();
        log.lock().unwrap().clear(); // ignore the set event
        m.get_secret("k").await.unwrap();
        let events = log.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].summary.contains("secrets.get"));
        assert!(events[0].summary.contains("outcome=ok"));
    }

    #[tokio::test]
    async fn test_audit_get_emits_event_not_found() {
        let (m, log) = manager_with_log();
        let _ = m.get_secret("missing").await;
        let events = log.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].summary.contains("secrets.get"));
        assert!(events[0].summary.contains("not_found"));
    }

    #[tokio::test]
    async fn test_audit_delete_emits_event_ok() {
        let (m, log) = manager_with_log();
        m.set_secret("tmp", "val".to_string()).await.unwrap();
        log.lock().unwrap().clear();
        m.delete_secret("tmp").await.unwrap();
        let events = log.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].summary.contains("secrets.delete"));
        assert!(events[0].summary.contains("outcome=ok"));
    }

    #[tokio::test]
    async fn test_audit_delete_emits_event_not_found() {
        let (m, log) = manager_with_log();
        let _ = m.delete_secret("nope").await;
        let events = log.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].summary.contains("not_found"));
    }

    #[tokio::test]
    async fn test_no_audit_no_panic_without_sink() {
        // Without a sink, operations must still succeed without panicking.
        let m = InMemorySecretsManager::new();
        m.set_secret("k", "v".to_string()).await.unwrap();
        m.get_secret("k").await.unwrap();
        m.delete_secret("k").await.unwrap();
    }

    // ── event type fields ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_audit_event_type_custom() {
        let (m, log) = manager_with_log();
        m.set_secret("x", "y".to_string()).await.unwrap();
        let events = log.lock().unwrap();
        assert!(matches!(
            &events[0].event_type,
            SecurityEventType::Custom(s) if s == "secrets.set"
        ));
    }

    #[tokio::test]
    async fn test_audit_event_severity_info() {
        let (m, log) = manager_with_log();
        m.set_secret("x", "y".to_string()).await.unwrap();
        let events = log.lock().unwrap();
        assert!(matches!(&events[0].severity, Severity::Info));
    }

    // ── concurrent access ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_concurrent_writes_are_safe() {
        let m = Arc::new(InMemorySecretsManager::new());
        let mut handles = vec![];
        for i in 0..50u32 {
            let m2 = Arc::clone(&m);
            handles.push(tokio::spawn(async move {
                m2.set_secret(&format!("key-{i}"), format!("val-{i}"))
                    .await
                    .unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        let keys = m.list_keys().await.unwrap();
        assert_eq!(keys.len(), 50);
    }

    // ── error display ─────────────────────────────────────────────────────────

    #[test]
    fn test_error_display_not_found() {
        let e = SecretsError::NotFound {
            key: "my-key".to_string(),
        };
        assert_eq!(e.to_string(), "secret not found: my-key");
    }

    #[test]
    fn test_error_display_access_denied() {
        let e = SecretsError::AccessDenied {
            actor: "agent-007".to_string(),
            key: "launch-codes".to_string(),
        };
        assert_eq!(
            e.to_string(),
            "access denied for actor 'agent-007' on key 'launch-codes'"
        );
    }

    #[test]
    fn test_error_display_backend() {
        let e = SecretsError::Backend("connection refused".to_string());
        assert_eq!(e.to_string(), "secrets backend error: connection refused");
    }
}
