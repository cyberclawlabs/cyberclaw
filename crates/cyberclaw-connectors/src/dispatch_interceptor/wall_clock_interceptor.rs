//! Wall-clock deadline interceptor (GAP-3 fix).
//!
//! Background: prior to this interceptor, `dispatcher.rs::dispatch()`
//! unwrapped `RuntimeMode::Native` calls without any `tokio::time::timeout`
//! wrapper. A misbehaving in-process connector (notably the browser
//! connector talking to a hung CDP socket) could pin the dispatcher
//! forever. Process and Container modes were already wrapped via
//! `execute_with_dispatch_budget`, so this gap was Native-only.
//!
//! This interceptor sets `ctx.deadline` from the capability contract's
//! `timeouts.request_ms`, falling back to a 120 s default that matches the
//! Process/Container `DEFAULT_DISPATCH_TIMEOUT_MS`. The dispatcher then
//! folds the deadline into a `tokio::time::timeout` wrapper for the Native
//! path, restoring symmetry with Process/Container dispatch.

use std::time::{Duration, Instant};

use super::{DispatchCtx, DispatchInterceptor};

/// Default wall-clock budget when the capability contract is silent.
/// Matches `dispatcher::DEFAULT_DISPATCH_TIMEOUT_MS` (120 s) so Native and
/// Process modes share the same fallback.
const DEFAULT_WALL_CLOCK_MS: u64 = 120_000;

/// Ceiling on dispatch wall-clock to prevent contracts from declaring
/// effectively-unbounded timeouts. Matches `dispatcher::MAX_DISPATCH_TIMEOUT_MS`.
const MAX_WALL_CLOCK_MS: u64 = 600_000;

/// Compute the per-dispatch wall-clock duration. Mirrors
/// `dispatcher::compute_dispatch_timeout` so Native/Process/Container all
/// pick the same number for the same contract.
pub(crate) fn compute_dispatch_timeout(request_ms: Option<u64>) -> Duration {
    let raw = match request_ms {
        Some(n) if n > 0 => n,
        _ => DEFAULT_WALL_CLOCK_MS,
    };
    Duration::from_millis(raw.clamp(1, MAX_WALL_CLOCK_MS))
}

/// Default interceptor that populates `ctx.deadline` for every dispatch.
///
/// Stateless; one instance can be shared across all dispatchers via `Arc`.
#[derive(Debug, Default, Clone)]
pub struct WallClockInterceptor;

impl WallClockInterceptor {
    /// Build a new wall-clock interceptor with default 120s fallback.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl DispatchInterceptor for WallClockInterceptor {
    async fn before(&self, ctx: &mut DispatchCtx) -> anyhow::Result<()> {
        let timeout = compute_dispatch_timeout(ctx.capability_contract.timeouts.request_ms);
        ctx.deadline = Some(Instant::now() + timeout);
        Ok(())
    }

    async fn after(
        &self,
        ctx: &DispatchCtx,
        _result: &mut crate::types::CapabilityExecutionResult,
    ) {
        // Hot-path log: if we burned > 50% of the deadline, surface a warning
        // so operators can spot near-timeouts in observability streams.
        if let Some(deadline) = ctx.deadline {
            let total = deadline.saturating_duration_since(ctx.started_at);
            let used = ctx.started_at.elapsed();
            if total.as_millis() > 0 && used.as_millis() * 2 > total.as_millis() {
                tracing::warn!(
                    capability_id = %ctx.request.capability_id,
                    used_ms = used.as_millis() as u64,
                    deadline_ms = total.as_millis() as u64,
                    "WallClockInterceptor: capability used >50% of its wall-clock budget"
                );
            }
        }
    }
}
