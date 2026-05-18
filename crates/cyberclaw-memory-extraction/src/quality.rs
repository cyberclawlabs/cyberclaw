//! Quality evaluation trait.

use cyberclaw_core::case::QualityScores;

/// Trait for evaluating execution quality across 4 dimensions.
/// Returns partial scores; caller constructs CaseQuality with execution_id and timestamp.
#[async_trait::async_trait]
pub trait QualityEvaluator: Send + Sync {
    async fn evaluate(
        &self,
        execution_summary: &str,
        execution_artifacts: &[String],
    ) -> anyhow::Result<QualityScores>;
}
