/// Agent trust level used to adjust execution risk scoring.
///
/// # Variants
/// - `Trusted` — system or platform agents; risk is reduced by one level.
/// - `Standard` — default for all unclassified agents; no adjustment.
/// - `Restricted` — untrusted or third-party agents; risk is elevated by one level.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum AgentTrustLevel {
    /// System/platform agents. Risk reduced by one level.
    Trusted,
    /// Default trust. No risk adjustment.
    #[default]
    Standard,
    /// Untrusted/third-party agents. Risk elevated by one level.
    Restricted,
}

// ---------------------------------------------------------------------------
// AgentRegistry trait
// ---------------------------------------------------------------------------

/// Minimal read-only agent registry interface.
///
/// Used by [`HandoffConnector`] (and future callers) to verify that a target
/// agent is registered before a handoff is enqueued.  Defined in `cyberclaw-core`
/// so both the connector crate and higher-level crates can reference it without
/// circular dependencies.
#[async_trait::async_trait]
pub trait AgentRegistry: Send + Sync {
    /// Returns `true` if the agent identified by `id` is known to the registry.
    async fn exists(&self, id: &crate::ids::AgentId) -> bool;
}

/// No-op implementation of [`AgentRegistry`] that always returns `true`.
///
/// Suitable for tests and callers that do not need real agent-existence
/// enforcement (e.g. the dev trigger endpoint, unit tests for connectors).
#[derive(Debug, Default)]
pub struct NoopAgentRegistry;

#[async_trait::async_trait]
impl AgentRegistry for NoopAgentRegistry {
    async fn exists(&self, _id: &crate::ids::AgentId) -> bool {
        true
    }
}
