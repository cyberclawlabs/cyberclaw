use crate::enums::{CaseKind, CaseStatus};
use crate::identity::ActorRef;
use crate::ids::{CaseId, TenantId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Case {
    pub id: CaseId,
    pub title: String,
    pub summary: String,
    pub kind: CaseKind,
    pub status: CaseStatus,
    pub owner_tenant: TenantId,
    pub created_by: ActorRef,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub labels: Vec<String>,
    pub metadata: BTreeMap<String, serde_json::Value>,
}

/// Quality dimension weights (EverOS 4-dimension maturity scoring).
pub const WEIGHT_COMPLETENESS: f32 = 0.3;
pub const WEIGHT_EXECUTABILITY: f32 = 0.3;
pub const WEIGHT_EVIDENCE: f32 = 0.2;
pub const WEIGHT_CLARITY: f32 = 0.2;

/// Success/failure threshold (borrowed from EverOS).
pub const SUCCESS_THRESHOLD: f32 = 0.6;

/// Compute weighted overall score from 4 dimension scores.
pub fn compute_weighted_overall(
    completeness: f32,
    executability: f32,
    evidence: f32,
    clarity: f32,
) -> f32 {
    debug_assert!(
        (0.0..=1.0).contains(&completeness),
        "completeness out of range: {completeness}"
    );
    debug_assert!(
        (0.0..=1.0).contains(&executability),
        "executability out of range: {executability}"
    );
    debug_assert!(
        (0.0..=1.0).contains(&evidence),
        "evidence out of range: {evidence}"
    );
    debug_assert!(
        (0.0..=1.0).contains(&clarity),
        "clarity out of range: {clarity}"
    );
    completeness * WEIGHT_COMPLETENESS
        + executability * WEIGHT_EXECUTABILITY
        + evidence * WEIGHT_EVIDENCE
        + clarity * WEIGHT_CLARITY
}

/// Execution quality evaluation (independent associated record).
/// Borrows EverOS 4-dimension maturity scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseQuality {
    pub execution_id: crate::ids::ExecutionId,
    /// Task completion degree (0.0-1.0).
    pub completeness: f32,
    /// Step executability (0.0-1.0).
    pub executability: f32,
    /// Evidence sufficiency (0.0-1.0).
    pub evidence: f32,
    /// Expression clarity (0.0-1.0).
    pub clarity: f32,
    /// Weighted composite score.
    pub overall_score: f32,
    pub evaluated_at: chrono::DateTime<chrono::Utc>,
}

impl CaseQuality {
    /// Weighted composite score using shared constants.
    pub fn compute_overall(&self) -> f32 {
        compute_weighted_overall(
            self.completeness,
            self.executability,
            self.evidence,
            self.clarity,
        )
    }

    /// Success threshold using shared constant.
    pub fn is_success_case(&self) -> bool {
        self.overall_score >= SUCCESS_THRESHOLD
    }
}

/// Partial result from LLM evaluation (4 dimension scores only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityScores {
    pub completeness: f32,
    pub executability: f32,
    pub evidence: f32,
    pub clarity: f32,
}

impl QualityScores {
    /// Weighted composite using shared constants (same formula as CaseQuality).
    pub fn compute_overall(&self) -> f32 {
        compute_weighted_overall(
            self.completeness,
            self.executability,
            self.evidence,
            self.clarity,
        )
    }
}

/// Agent execution case record (EverOS `AgentCase`).
/// Captures task_intent, approach, and key_insight for skill evolution clustering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCaseRecord {
    pub execution_id: crate::ids::ExecutionId,
    /// What was the agent trying to accomplish.
    pub task_intent: String,
    /// How did the agent approach the task.
    pub approach: String,
    /// Quality score from CaseQuality evaluation.
    pub quality_score: f32,
    /// Key lesson or insight extracted from this execution.
    pub key_insight: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Storage trait for CaseQuality records.
#[async_trait::async_trait]
pub trait CaseQualityStore: Send + Sync {
    async fn store(&self, quality: CaseQuality) -> anyhow::Result<()>;
    async fn get_by_execution(
        &self,
        id: &crate::ids::ExecutionId,
    ) -> anyhow::Result<Option<CaseQuality>>;
    async fn list_recent(&self, limit: usize) -> anyhow::Result<Vec<CaseQuality>>;
}

#[cfg(test)]
mod quality_tests {
    use super::*;

    #[test]
    fn test_case_quality_compute_overall() {
        let q = CaseQuality {
            execution_id: crate::ids::ExecutionId::from_string("exec-001".to_string()).unwrap(),
            completeness: 0.8,
            executability: 0.9,
            evidence: 0.7,
            clarity: 0.6,
            overall_score: 0.0,
            evaluated_at: chrono::Utc::now(),
        };
        let score = q.compute_overall();
        // 0.8*0.3 + 0.9*0.3 + 0.7*0.2 + 0.6*0.2 = 0.24 + 0.27 + 0.14 + 0.12 = 0.77
        assert!((score - 0.77).abs() < 0.01);
    }

    #[test]
    fn test_case_quality_is_success() {
        let mut q = CaseQuality {
            execution_id: crate::ids::ExecutionId::from_string("exec-002".to_string()).unwrap(),
            completeness: 0.8,
            executability: 0.8,
            evidence: 0.8,
            clarity: 0.8,
            overall_score: 0.8,
            evaluated_at: chrono::Utc::now(),
        };
        assert!(q.is_success_case());

        q.overall_score = 0.3;
        assert!(!q.is_success_case());
    }

    #[test]
    fn test_case_quality_serde_roundtrip() {
        let q = CaseQuality {
            execution_id: crate::ids::ExecutionId::from_string("exec-003".to_string()).unwrap(),
            completeness: 0.9,
            executability: 0.85,
            evidence: 0.7,
            clarity: 0.95,
            overall_score: 0.86,
            evaluated_at: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&q).expect("serialize");
        let deser: CaseQuality = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.execution_id, q.execution_id);
        assert!((deser.completeness - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn test_quality_scores_partial() {
        let s = QualityScores {
            completeness: 1.0,
            executability: 1.0,
            evidence: 1.0,
            clarity: 1.0,
        };
        let overall = s.compute_overall();
        assert!((overall - 1.0).abs() < f32::EPSILON);
    }
}
