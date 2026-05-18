//! AtomicFact extraction trait and types.

use crate::memcell::MemCell;
use chrono::{DateTime, NaiveDate, Utc};
use cyberclaw_core::ids::{ActorId, FactId, MemCellId};
use serde::{Deserialize, Serialize};

/// Atomic fact -- indivisible knowledge unit extracted from conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtomicFact {
    pub id: FactId,
    pub content: String,
    pub source_memcell_id: MemCellId,
    pub actor_id: Option<ActorId>,
    pub confidence: f32,
    pub temporal: Option<TemporalContext>,
    pub extracted_at: DateTime<Utc>,
}

/// Temporal context for time-bound facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalContext {
    pub reference_date: Option<NaiveDate>,
    /// Human-readable time description from LLM (e.g. "March 10, 2024(Sunday) at 2:00 PM").
    /// Preserves EverOS `time` field semantics including weekday and time-of-day.
    #[serde(default)]
    pub time_description: Option<String>,
    pub is_ongoing: bool,
}

/// Trait for extracting atomic facts from a MemCell.
#[async_trait::async_trait]
pub trait AtomicFactExtractor: Send + Sync {
    async fn extract(
        &self,
        memcell: &MemCell,
        target_actor: Option<&ActorId>,
    ) -> anyhow::Result<Vec<AtomicFact>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atomic_fact_serde_roundtrip() {
        let fact = AtomicFact {
            id: FactId::from_string("fact-001".to_string()).unwrap(),
            content: "User prefers morning meetings".to_string(),
            source_memcell_id: MemCellId::from_string("mc-001".to_string()).unwrap(),
            actor_id: None,
            confidence: 0.85,
            temporal: Some(TemporalContext {
                reference_date: Some(NaiveDate::from_ymd_opt(2026, 4, 15).unwrap()),
                time_description: Some("April 15, 2026(Tuesday) at 10:00 AM".to_string()),
                is_ongoing: true,
            }),
            extracted_at: Utc::now(),
        };
        let json = serde_json::to_string(&fact).expect("serialize");
        let deser: AtomicFact = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.content, "User prefers morning meetings");
        assert!(deser.temporal.unwrap().is_ongoing);
    }
}
