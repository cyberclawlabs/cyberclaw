use crate::error::{ConsensusError, Result};
use crate::raft::{node::RaftNode, state::NodeId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusState {
    pub is_leader: bool,
    pub current_term: u64,
    pub leader_id: Option<NodeId>,
    pub commit_index: u64,
    pub last_applied: u64,
}

/// Generic consensus interface that can be implemented by different algorithms
#[async_trait]
pub trait Consensus: Send + Sync {
    /// Propose a command to be added to the replicated log
    async fn propose(&self, command: Vec<u8>) -> Result<u64>;

    /// Apply a committed entry from the log
    async fn apply(&self, index: u64) -> Result<Vec<u8>>;

    /// Get the current leader
    async fn get_leader(&self) -> Option<NodeId>;

    /// Get the current consensus state
    async fn get_state(&self) -> ConsensusState;

    /// Check if this node is the leader
    async fn is_leader(&self) -> bool;

    /// Get the current term
    async fn get_term(&self) -> u64;

    /// Shutdown the consensus module
    async fn shutdown(&self) -> Result<()>;
}

/// Raft-based consensus implementation
pub struct RaftConsensus {
    node: Arc<RaftNode>,
}

impl RaftConsensus {
    pub fn new(node: Arc<RaftNode>) -> Self {
        Self { node }
    }

    pub async fn start(&self) -> Result<()> {
        self.node.start().await
    }

    pub fn node_handle(&self) -> Arc<RaftNode> {
        self.node.clone()
    }
}

#[async_trait]
impl Consensus for RaftConsensus {
    async fn propose(&self, command: Vec<u8>) -> Result<u64> {
        self.node.propose(command).await
    }

    async fn apply(&self, index: u64) -> Result<Vec<u8>> {
        let log = self.node.log.read().await;

        if let Some(entry) = log.get(index) {
            Ok(entry.command.clone())
        } else {
            Err(ConsensusError::LogCompacted(index))
        }
    }

    async fn get_leader(&self) -> Option<NodeId> {
        let state = self.node.state.read().await;
        state.leader_id.clone()
    }

    async fn get_state(&self) -> ConsensusState {
        let state = self.node.state.read().await;
        let log = self.node.log.read().await;

        ConsensusState {
            is_leader: state.role == crate::raft::state::RaftRole::Leader,
            current_term: state.current_term,
            leader_id: state.leader_id.clone(),
            commit_index: log.commit_index(),
            last_applied: log.last_applied(),
        }
    }

    async fn is_leader(&self) -> bool {
        let state = self.node.state.read().await;
        state.role == crate::raft::state::RaftRole::Leader
    }

    async fn get_term(&self) -> u64 {
        let state = self.node.state.read().await;
        state.current_term
    }

    async fn shutdown(&self) -> Result<()> {
        self.node.shutdown().await
    }
}

/// Builder for creating consensus instances
pub struct ConsensusBuilder {
    node_id: NodeId,
    peers: Vec<crate::raft::node::PeerInfo>,
    config: Option<crate::raft::node::RaftConfig>,
}

impl ConsensusBuilder {
    pub fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            peers: Vec::new(),
            config: None,
        }
    }

    pub fn add_peer(mut self, id: NodeId, address: String) -> Self {
        self.peers.push(crate::raft::node::PeerInfo { id, address });
        self
    }

    pub fn with_peers(mut self, peers: Vec<crate::raft::node::PeerInfo>) -> Self {
        self.peers = peers;
        self
    }

    pub fn with_config(mut self, config: crate::raft::node::RaftConfig) -> Self {
        self.config = Some(config);
        self
    }

    pub fn build_raft(self) -> RaftConsensus {
        let config = self.config.unwrap_or_default();
        let node = Arc::new(RaftNode::new(self.node_id, self.peers, config));
        RaftConsensus::new(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raft::node::RaftConfig;

    #[tokio::test]
    async fn test_consensus_builder() {
        let consensus = ConsensusBuilder::new("node1".to_string())
            .add_peer("node2".to_string(), "127.0.0.1:9002".to_string())
            .add_peer("node3".to_string(), "127.0.0.1:9003".to_string())
            .with_config(RaftConfig {
                election_timeout_min: 100,
                election_timeout_max: 200,
                heartbeat_interval: 30,
                max_append_entries: 50,
            })
            .build_raft();

        let state = consensus.get_state().await;
        assert!(!state.is_leader);
        assert_eq!(state.current_term, 0);
    }

    #[tokio::test]
    async fn test_consensus_state() {
        let node = Arc::new(RaftNode::new(
            "test".to_string(),
            vec![],
            RaftConfig::default(),
        ));
        let consensus = RaftConsensus::new(node);

        let state = consensus.get_state().await;
        assert!(!state.is_leader);
        assert_eq!(state.commit_index, 0);
        assert_eq!(state.last_applied, 0);
    }

    #[tokio::test]
    async fn test_is_leader() {
        let node = Arc::new(RaftNode::new(
            "test".to_string(),
            vec![],
            RaftConfig::default(),
        ));
        let consensus = RaftConsensus::new(node);

        assert!(!consensus.is_leader().await);
    }
}
