//! MemCell types and BoundaryDetector trait.
//!
//! A MemCell represents a coherent segment of conversation suitable for memory extraction.
//! Borrowed from EverOS ConvMemCellExtractor / AgentMemCellExtractor.

use chrono::{DateTime, Utc};
use cyberclaw_core::ids::{ActorId, MemCellId};
use serde::{Deserialize, Serialize};

/// Raw message in a conversation (simplified for extraction pipeline).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMessage {
    pub sender_id: Option<String>,
    pub sender_name: Option<String>,
    pub role: String,
    pub content: String,
    pub timestamp: Option<DateTime<Utc>>,
}

/// A single conversation turn (filtered: tool calls excluded for agent conversations).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub role: String,
    pub content: String,
    pub sender_id: Option<String>,
    pub timestamp: Option<DateTime<Utc>>,
}

/// Raw data type for boundary detection dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RawDataType {
    Conversation,
    AgentConversation,
}

/// Conversation semantic boundary detection unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemCell {
    pub id: MemCellId,
    pub conversation_data: Vec<ConversationTurn>,
    pub original_data: Vec<RawMessage>,
    pub raw_data_type: RawDataType,
    pub timestamp: DateTime<Utc>,
    /// Display-name participants (EverOS `participants`).
    pub participant_ids: Vec<ActorId>,
    /// Owner user IDs for multi-tenant isolation (EverOS `user_id_list`).
    pub user_id_list: Vec<String>,
    /// Group/channel identifier (EverOS `group_id`).
    #[serde(default)]
    pub group_id: Option<String>,
    /// Raw sender IDs distinct from display participants (EverOS `sender_ids`).
    #[serde(default)]
    pub sender_ids: Vec<String>,
}

/// Boundary detection status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryStatus {
    /// Boundary detected, MemCells ready.
    Ready,
    /// Not enough data, accumulating.
    Accumulating,
    /// Flush forced remaining into final MemCell.
    Flushed,
}

/// Result of boundary detection.
pub struct BoundaryResult {
    pub memcells: Vec<MemCell>,
    pub status: BoundaryStatus,
}

/// Trait for conversation boundary detection.
#[async_trait::async_trait]
pub trait BoundaryDetector: Send + Sync {
    /// Detect conversation boundaries and produce MemCells.
    async fn detect(
        &self,
        history: &[RawMessage],
        new_messages: &[RawMessage],
        flush: bool,
    ) -> anyhow::Result<BoundaryResult>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memcell_creation() {
        let cell = MemCell {
            id: MemCellId::new(),
            conversation_data: vec![],
            original_data: vec![],
            raw_data_type: RawDataType::Conversation,
            timestamp: Utc::now(),
            participant_ids: vec![],
            user_id_list: vec!["user-1".to_string()],
            group_id: None,
            sender_ids: vec![],
        };
        assert!(cell.conversation_data.is_empty());
        assert_eq!(cell.raw_data_type, RawDataType::Conversation);
    }

    #[test]
    fn test_boundary_status_values() {
        assert_ne!(BoundaryStatus::Ready, BoundaryStatus::Accumulating);
        assert_ne!(BoundaryStatus::Ready, BoundaryStatus::Flushed);
    }

    #[test]
    fn test_memcell_serde_roundtrip() {
        let cell = MemCell {
            id: MemCellId::from_string("mc-001".to_string()).unwrap(),
            conversation_data: vec![ConversationTurn {
                role: "user".to_string(),
                content: "Hello".to_string(),
                sender_id: Some("user-1".to_string()),
                timestamp: Some(Utc::now()),
            }],
            original_data: vec![],
            raw_data_type: RawDataType::AgentConversation,
            timestamp: Utc::now(),
            participant_ids: vec![],
            user_id_list: vec![],
            group_id: Some("group-1".to_string()),
            sender_ids: vec!["sender-1".to_string()],
        };
        let json = serde_json::to_string(&cell).expect("serialize");
        let deser: MemCell = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.id.as_str(), "mc-001");
        assert_eq!(deser.conversation_data.len(), 1);
    }
}
