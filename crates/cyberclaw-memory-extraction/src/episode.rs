//! Episode extraction trait and types.

use crate::memcell::MemCell;
use chrono::{DateTime, Utc};
use cyberclaw_core::ids::{ActorId, MemCellId};
use serde::{Deserialize, Serialize};

/// LLM-extracted episode (EverOS: subject + summary + episode).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedEpisode {
    /// Subject/title of the episode.
    pub title: String,
    /// Short summary (fallback: first 200 chars of episode_content).
    pub summary: String,
    /// Full narrative content (EverOS `episode` field — the core data).
    pub episode_content: String,
    pub key_participants: Vec<ActorId>,
    pub timestamp: DateTime<Utc>,
    pub source_memcell_id: MemCellId,
}

/// Trait for extracting episode summaries from a MemCell.
#[async_trait::async_trait]
pub trait EpisodeExtractor: Send + Sync {
    /// Extract episode summary. Supports group (actor_id=None) and personal.
    async fn extract(
        &self,
        memcell: &MemCell,
        actor_id: Option<&ActorId>,
        group_id: Option<&str>,
    ) -> anyhow::Result<Option<ExtractedEpisode>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extracted_episode_serde() {
        let ep = ExtractedEpisode {
            title: "Project kickoff meeting".to_string(),
            summary: "Team discussed Q2 goals".to_string(),
            episode_content: "The team met on Monday to discuss Q2 goals. Alice proposed focusing on performance. Bob suggested adding monitoring. The team agreed on both priorities.".to_string(),
            key_participants: vec![],
            timestamp: Utc::now(),
            source_memcell_id: MemCellId::from_string("mc-001".to_string()).unwrap(),
        };
        let json = serde_json::to_string(&ep).expect("serialize");
        let deser: ExtractedEpisode = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.title, "Project kickoff meeting");
    }
}
