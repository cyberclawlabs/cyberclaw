//! SkillHandler trait definition.

use async_trait::async_trait;

/// A handler that processes skill invocations.
///
/// Implement this trait to define custom skill logic.
#[async_trait]
pub trait SkillHandler: Send + Sync {
    /// Execute the skill with the given JSON input and return a JSON output.
    async fn handle(&self, input: serde_json::Value) -> anyhow::Result<serde_json::Value>;
}
