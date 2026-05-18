//! AutoModeGate — enter/exit auto-mode with permission snapshotting.
//!
//! Tracks which executions are currently running in auto mode and records a
//! [`PermissionSnapshot`] for each.  The snapshot carries the capabilities that
//! were stripped on entry and can be used by callers to restore state on exit.
//!
//! The dangerous-capability filtering step (populating `stripped_capabilities`)
//! is intentionally left as an integration seam: `enter_auto_mode` stores an
//! empty `stripped_capabilities` vec.  A higher-level orchestrator that holds a
//! `DangerousCapabilityFilter` should call into that filter before or after
//! constructing the snapshot and amend `stripped_capabilities` as needed.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cyberclaw_core::ids::ExecutionId;
use cyberclaw_governance::{CapabilityException, DangerSeverity};
use tracing::{info, warn};

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration controlling auto-mode behaviour for a single execution.
#[derive(Debug, Clone)]
pub struct AutoModeConfig {
    /// Whether auto mode is actually enabled.
    pub enabled: bool,
    /// Number of consecutive failures that trip the circuit breaker.
    pub circuit_breaker_threshold: u32,
    /// Cool-down period after the circuit breaker trips before retrying.
    pub circuit_breaker_cooldown: Duration,
    /// Capability patterns that are explicitly allowed despite normal filtering.
    pub capability_exceptions: Vec<CapabilityException>,
    /// Maximum number of autonomous iterations before forcing exit.
    pub max_auto_iterations: u32,
}

impl Default for AutoModeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            circuit_breaker_threshold: 3,
            circuit_breaker_cooldown: Duration::from_secs(60),
            capability_exceptions: Vec::new(),
            max_auto_iterations: 50,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Snapshot types
// ─────────────────────────────────────────────────────────────────────────────

/// Immutable record of the permission state at the moment auto mode was entered.
#[derive(Debug, Clone)]
pub struct PermissionSnapshot {
    /// Wall-clock time when the snapshot was created.
    pub created_at: Instant,
    /// Capabilities that were stripped when entering auto mode.
    pub stripped_capabilities: Vec<StrippedCapability>,
    /// Serialised form of the [`AutoModeConfig`] that was in effect.
    pub original_config: serde_json::Value,
}

/// Record of a single capability that was removed on auto-mode entry.
#[derive(Debug, Clone)]
pub struct StrippedCapability {
    /// Identifier of the stripped capability.
    pub capability_id: String,
    /// The rule that matched this capability.
    pub matched_rule: String,
    /// How dangerous the capability was classified as.
    pub severity: DangerSeverity,
}

// ─────────────────────────────────────────────────────────────────────────────
// Exit reason
// ─────────────────────────────────────────────────────────────────────────────

/// Reason recorded when auto mode is exited.
#[derive(Debug, Clone)]
pub enum ExitReason {
    /// The user explicitly ended auto mode.
    UserRequested,
    /// The autonomous goal was achieved successfully.
    GoalMet,
    /// Too many consecutive failures triggered the circuit breaker.
    CircuitBreak { consecutive_failures: u32 },
    /// The configured iteration ceiling was reached.
    MaxIterationsReached { iterations: u32 },
    /// The execution budget (cost / tokens / time) was exhausted.
    BudgetExhausted,
    /// A security gate vetoed continued execution.
    SecurityGateBlock { reason: String },
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait
// ─────────────────────────────────────────────────────────────────────────────

/// Lifecycle gate for auto-mode executions.
///
/// Implementors are responsible for tracking which executions are currently
/// running in auto mode and for persisting / restoring permission snapshots.
#[async_trait]
pub trait AutoModeGate: Send + Sync {
    /// Enter auto mode for `execution_id` under `config`.
    ///
    /// Returns a [`PermissionSnapshot`] capturing the state at entry.  The
    /// caller should store the snapshot and pass it back to [`exit_auto_mode`]
    /// when the session ends.
    async fn enter_auto_mode(
        &self,
        execution_id: &ExecutionId,
        config: &AutoModeConfig,
    ) -> anyhow::Result<PermissionSnapshot>;

    /// Exit auto mode for `execution_id`, restoring state from `snapshot`.
    ///
    /// The `reason` is recorded in structured logs and may be forwarded to an
    /// audit sink by the implementation.
    async fn exit_auto_mode(
        &self,
        execution_id: &ExecutionId,
        snapshot: &PermissionSnapshot,
        reason: ExitReason,
    ) -> anyhow::Result<()>;

    /// Returns `true` if `execution_id` is currently running in auto mode.
    fn is_auto_mode(&self, execution_id: &ExecutionId) -> bool;
}

// ─────────────────────────────────────────────────────────────────────────────
// Default implementation
// ─────────────────────────────────────────────────────────────────────────────

/// In-process implementation of [`AutoModeGate`] backed by a `RwLock<HashMap>`.
///
/// Sessions are keyed by the string representation of [`ExecutionId`].
/// Entering auto mode for an already-active `execution_id` **overwrites** the
/// previous snapshot rather than panicking.
#[derive(Debug, Default)]
pub struct DefaultAutoModeGate {
    sessions: RwLock<HashMap<String, PermissionSnapshot>>,
}

impl DefaultAutoModeGate {
    /// Create a new, empty gate.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl AutoModeGate for DefaultAutoModeGate {
    async fn enter_auto_mode(
        &self,
        execution_id: &ExecutionId,
        config: &AutoModeConfig,
    ) -> anyhow::Result<PermissionSnapshot> {
        let key = execution_id.to_string();

        let original_config = serde_json::json!({
            "enabled": config.enabled,
            "circuit_breaker_threshold": config.circuit_breaker_threshold,
            "circuit_breaker_cooldown_secs": config.circuit_breaker_cooldown.as_secs(),
            "max_auto_iterations": config.max_auto_iterations,
            "capability_exceptions_count": config.capability_exceptions.len(),
        });

        let snapshot = PermissionSnapshot {
            created_at: Instant::now(),
            // Integration seam: DangerousCapabilityFilter fills this in when wired up.
            stripped_capabilities: Vec::new(),
            original_config,
        };

        {
            let mut sessions = self
                .sessions
                .write()
                .map_err(|_| anyhow::anyhow!("auto_mode_gate: sessions lock poisoned"))?;
            sessions.insert(key.clone(), snapshot.clone());
        }

        info!(
            execution_id = %key,
            enabled = config.enabled,
            max_iterations = config.max_auto_iterations,
            "auto mode entered",
        );

        Ok(snapshot)
    }

    async fn exit_auto_mode(
        &self,
        execution_id: &ExecutionId,
        snapshot: &PermissionSnapshot,
        reason: ExitReason,
    ) -> anyhow::Result<()> {
        let key = execution_id.to_string();

        let removed = {
            let mut sessions = self
                .sessions
                .write()
                .map_err(|_| anyhow::anyhow!("auto_mode_gate: sessions lock poisoned"))?;
            sessions.remove(&key)
        };

        if removed.is_none() {
            warn!(
                execution_id = %key,
                "exit_auto_mode called for execution that was not in auto mode",
            );
        }

        let elapsed = snapshot.created_at.elapsed();

        info!(
            execution_id = %key,
            elapsed_ms = elapsed.as_millis(),
            reason = ?reason,
            "auto mode exited",
        );

        Ok(())
    }

    fn is_auto_mode(&self, execution_id: &ExecutionId) -> bool {
        let key = execution_id.to_string();
        self.sessions
            .read()
            .map(|s| s.contains_key(&key))
            .unwrap_or(false)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::ids::ExecutionId;

    fn make_id() -> ExecutionId {
        ExecutionId::new()
    }

    fn default_config() -> AutoModeConfig {
        AutoModeConfig::default()
    }

    #[tokio::test]
    async fn enter_sets_auto_mode() {
        let gate = DefaultAutoModeGate::new();
        let id = make_id();

        assert!(
            !gate.is_auto_mode(&id),
            "should not be in auto mode before enter"
        );

        gate.enter_auto_mode(&id, &default_config())
            .await
            .expect("enter_auto_mode should succeed");

        assert!(gate.is_auto_mode(&id), "should be in auto mode after enter");
    }

    #[tokio::test]
    async fn exit_clears_auto_mode() {
        let gate = DefaultAutoModeGate::new();
        let id = make_id();

        let snapshot = gate
            .enter_auto_mode(&id, &default_config())
            .await
            .expect("enter_auto_mode should succeed");

        assert!(gate.is_auto_mode(&id));

        gate.exit_auto_mode(&id, &snapshot, ExitReason::GoalMet)
            .await
            .expect("exit_auto_mode should succeed");

        assert!(
            !gate.is_auto_mode(&id),
            "should not be in auto mode after exit"
        );
    }

    #[tokio::test]
    async fn repeated_enter_overwrites_without_panic() {
        let gate = DefaultAutoModeGate::new();
        let id = make_id();

        gate.enter_auto_mode(&id, &default_config())
            .await
            .expect("first enter should succeed");

        // Second enter with a different config — must not panic or error.
        let mut config2 = default_config();
        config2.max_auto_iterations = 10;

        let snapshot2 = gate
            .enter_auto_mode(&id, &config2)
            .await
            .expect("second enter should succeed without panic");

        assert!(
            gate.is_auto_mode(&id),
            "still in auto mode after second enter"
        );

        // Exit using the latest snapshot.
        gate.exit_auto_mode(&id, &snapshot2, ExitReason::UserRequested)
            .await
            .expect("exit after second enter should succeed");

        assert!(!gate.is_auto_mode(&id));
    }
}
