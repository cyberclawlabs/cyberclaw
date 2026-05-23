//! # DispatchInterceptor — unified pre/post hook for capability dispatch
//!
//! Solves four quality gaps identified in the v1.x business matrix:
//!
//! 1. **GAP-1 (B-class shell counting)**: tool-result truncation markers are
//!    plain strings (`...[truncated N bytes]...`) that the model parses
//!    unreliably. [`TruncationMetadataInterceptor`] injects a structured
//!    `_meta.truncated = true` field so the LLM gets a deterministic signal.
//! 2. **GAP-2 (C-L2/L3 sandbox bypass)**: only `local/cmd.rs` consults
//!    `SandboxProfile`. [`SandboxInjectionInterceptor`] propagates a
//!    `sandbox_hint` based on capability ID so future connectors can opt-in
//!    via the `DispatchCtx` without re-implementing rule lookup.
//! 3. **GAP-3 (D-class browser hang)**: the Native dispatch branch in
//!    `dispatcher.rs` was unwrapped (no timeout!). [`WallClockInterceptor`]
//!    computes a deadline so the dispatcher can fold it into the
//!    `tokio::time::timeout` wrapper for *all* runtime modes.
//! 4. **GAP-4 (F-L2 wall-clock)** is fixed in `loop_governor.rs` (raised to
//!    180 s); the interceptor architecture is unrelated to that change but
//!    shipped in the same wave.
//!
//! ## Design constraint
//!
//! Interceptors are run sequentially. `before(...)` may mutate the
//! [`DispatchCtx`] (set deadline, set sandbox_hint). `after(...)` is given
//! read-only access to the context and a mutable result so it can annotate
//! `result.output` with metadata. Errors from `before(...)` abort dispatch;
//! `after(...)` is infallible (annotation should never fail the call).

use std::time::Instant;

use crate::types::{CapabilityExecutionRequest, CapabilityExecutionResult};
use cyberclaw_core::manifests::CapabilityContract;

pub mod sandbox_injection_interceptor;
pub mod truncation_metadata_interceptor;
pub mod wall_clock_interceptor;

pub use sandbox_injection_interceptor::SandboxInjectionInterceptor;
pub use truncation_metadata_interceptor::TruncationMetadataInterceptor;
pub use wall_clock_interceptor::WallClockInterceptor;

/// Mutable per-dispatch context shared across all interceptors and the
/// dispatcher itself.
///
/// Fields are populated cumulatively as each interceptor's `before` runs:
/// - `deadline` is owned by [`WallClockInterceptor`] (GAP-3 fix).
/// - `sandbox_hint` is owned by [`SandboxInjectionInterceptor`] (GAP-2 hook).
/// - `started_at` is set once at construction by the dispatcher.
///
/// The struct intentionally holds clones of the request and the contract so
/// interceptors can inspect them without borrowing the dispatcher.
#[derive(Debug, Clone)]
pub struct DispatchCtx {
    /// Snapshot of the inbound request. Interceptors must NOT mutate this
    /// (only the dispatcher dispatches the real request).
    pub request: CapabilityExecutionRequest,
    /// Snapshot of the resolved capability contract.
    pub capability_contract: CapabilityContract,
    /// Absolute deadline by which `connector.execute()` must finish. `None`
    /// means "no per-interceptor deadline" (dispatcher falls back to default).
    pub deadline: Option<Instant>,
    /// Serialized [`crate::sandbox::SandboxProfileId`] name (`"dev"`,
    /// `"minimal"`, `"isolated"`). `None` for in-process Rust ops that don't
    /// need sandbox enforcement.
    pub sandbox_hint: Option<String>,
    /// Wall-clock start of dispatch. Used by `after(...)` hooks that need
    /// to compute elapsed time without re-reading `Instant::now()`.
    pub started_at: Instant,
}

impl DispatchCtx {
    /// Build a fresh context for a dispatch operation.
    pub fn new(request: CapabilityExecutionRequest, capability_contract: CapabilityContract) -> Self {
        Self {
            request,
            capability_contract,
            deadline: None,
            sandbox_hint: None,
            started_at: Instant::now(),
        }
    }
}

/// Pre/post hook contract for capability dispatch.
///
/// Implementations must be `Send + Sync` because the dispatcher holds them
/// behind `Arc<dyn DispatchInterceptor>` and may invoke them from tasks
/// spawned across the tokio executor.
#[async_trait::async_trait]
pub trait DispatchInterceptor: Send + Sync {
    /// Runs before `connector.execute()`. May set deadline, sandbox_hint,
    /// or short-circuit dispatch by returning `Err(...)`. Multiple
    /// interceptors run sequentially in registration order.
    async fn before(&self, ctx: &mut DispatchCtx) -> anyhow::Result<()>;

    /// Runs after `connector.execute()` (success path only). May annotate
    /// `result.output` with metadata. Errors here are not propagated — the
    /// `after` hook MUST NOT fail the dispatch.
    async fn after(&self, ctx: &DispatchCtx, result: &mut CapabilityExecutionResult);
}

#[cfg(test)]
mod tests;
