//! Sandbox-hint injection interceptor (GAP-2 fix).
//!
//! Background: only `local/cmd.rs` consults [`crate::sandbox::SandboxProfile`].
//! 16 sibling connectors (fs, search, lsp, web, exec_runtime, …) bypass
//! sandbox enforcement entirely. A full fix requires per-connector opt-in,
//! which is a large refactor and out of scope for this wave.
//!
//! What this interceptor does ship is the architectural plumbing: it tags
//! `ctx.sandbox_hint` with a serialized [`crate::sandbox::SandboxProfileId`]
//! name (`"dev"`, `"minimal"`, `"isolated"`) derived from the capability ID
//! prefix. Connectors that already honour the hint (cmd.rs today, others
//! in future waves) get correct sandbox routing automatically. Connectors
//! that ignore the hint behave unchanged.
//!
//! ## Routing table
//!
//! | Capability prefix | Sandbox hint | Rationale                              |
//! |-------------------|--------------|----------------------------------------|
//! | `cmd.`            | `dev`        | Host shell needs writable workspace    |
//! | `exec.`           | `dev`        | Same as cmd.                           |
//! | `search.`         | `minimal`    | Read-only ripgrep/glob                 |
//! | `fs.`             | `minimal`    | Filesystem read; writes are explicit   |
//! | `web.`            | `isolated`   | Network calls need strict isolation    |
//! | other             | `None`       | Pure Rust ops don't need sandboxing    |

use super::{DispatchCtx, DispatchInterceptor};

/// Sandbox profile name returned for `cmd.*` / `exec.*` capabilities.
pub const HINT_DEV: &str = "dev";
/// Sandbox profile name returned for `fs.*` / `search.*` (read-only) capabilities.
pub const HINT_MINIMAL: &str = "minimal";
/// Sandbox profile name returned for `web.*` capabilities (network-bound).
pub const HINT_ISOLATED: &str = "isolated";

/// Stateless interceptor that maps capability ID prefixes to sandbox hints.
#[derive(Debug, Default, Clone)]
pub struct SandboxInjectionInterceptor;

impl SandboxInjectionInterceptor {
    /// Build a new interceptor instance.
    pub fn new() -> Self {
        Self
    }

    /// Pure function: given a capability ID, return the sandbox hint.
    ///
    /// Exposed for unit testing without constructing a full `DispatchCtx`.
    pub fn pick_sandbox_hint(capability_id: &str) -> Option<&'static str> {
        // Order: most-restrictive first. Returns on first match.
        const TABLE: &[(&str, &str)] = &[
            ("cmd.", HINT_DEV),
            ("exec.", HINT_DEV),
            ("search.", HINT_MINIMAL),
            ("fs.", HINT_MINIMAL),
            ("web.", HINT_ISOLATED),
        ];
        TABLE
            .iter()
            .find(|(prefix, _)| capability_id.starts_with(prefix))
            .map(|(_, hint)| *hint)
    }
}

#[async_trait::async_trait]
impl DispatchInterceptor for SandboxInjectionInterceptor {
    async fn before(&self, ctx: &mut DispatchCtx) -> anyhow::Result<()> {
        ctx.sandbox_hint =
            Self::pick_sandbox_hint(ctx.request.capability_id.as_str()).map(String::from);
        Ok(())
    }

    async fn after(
        &self,
        _ctx: &DispatchCtx,
        _result: &mut crate::types::CapabilityExecutionResult,
    ) {
    }
}
