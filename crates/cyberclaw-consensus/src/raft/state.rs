use serde::{Deserialize, Serialize};
use std::fmt;

pub type NodeId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RaftRole {
    Follower,
    Candidate,
    Leader,
}

impl fmt::Display for RaftRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RaftRole::Follower => write!(f, "Follower"),
            RaftRole::Candidate => write!(f, "Candidate"),
            RaftRole::Leader => write!(f, "Leader"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaftState {
    /// Current term
    pub current_term: u64,

    /// Node ID that received vote in current term
    pub voted_for: Option<NodeId>,

    /// Current role of the node
    pub role: RaftRole,

    /// Current leader ID
    pub leader_id: Option<NodeId>,

    /// Number of votes received in current term (for candidates)
    pub votes_received: usize,
}

impl RaftState {
    pub fn new() -> Self {
        Self {
            current_term: 0,
            voted_for: None,
            role: RaftRole::Follower,
            leader_id: None,
            votes_received: 0,
        }
    }

    /// Transition to follower state
    pub fn become_follower(&mut self, term: u64) {
        self.current_term = term;
        self.voted_for = None;
        self.role = RaftRole::Follower;
        self.votes_received = 0;
        tracing::info!("Node became follower at term {}", term);
    }

    /// Transition to candidate state
    pub fn become_candidate(&mut self) {
        self.current_term += 1;
        self.role = RaftRole::Candidate;
        self.voted_for = None; // Will vote for self
        self.votes_received = 0;
        self.leader_id = None;
        tracing::info!("Node became candidate at term {}", self.current_term);
    }

    /// Transition to leader state
    pub fn become_leader(&mut self) {
        self.role = RaftRole::Leader;
        self.leader_id = self.voted_for.clone(); // Leader is self
        tracing::info!("Node became leader at term {}", self.current_term);
    }

    /// Vote for a candidate
    pub fn vote_for(&mut self, candidate_id: NodeId) {
        self.voted_for = Some(candidate_id);
    }

    /// Increment votes received (for candidates)
    pub fn receive_vote(&mut self) {
        self.votes_received += 1;
    }

    /// Check if can vote for candidate
    pub fn can_vote_for(&self, candidate_id: &str) -> bool {
        self.voted_for.is_none() || self.voted_for.as_ref() == Some(&candidate_id.to_string())
    }

    /// Update term if needed
    pub fn maybe_update_term(&mut self, term: u64) -> bool {
        if term > self.current_term {
            self.become_follower(term);
            true
        } else {
            false
        }
    }
}

impl Default for RaftState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_transitions() {
        let mut state = RaftState::new();
        assert_eq!(state.role, RaftRole::Follower);
        assert_eq!(state.current_term, 0);

        // Become candidate
        state.become_candidate();
        assert_eq!(state.role, RaftRole::Candidate);
        assert_eq!(state.current_term, 1);

        // Become leader
        state.vote_for("node1".to_string());
        state.become_leader();
        assert_eq!(state.role, RaftRole::Leader);
        assert_eq!(state.leader_id, Some("node1".to_string()));

        // Step down to follower
        state.become_follower(5);
        assert_eq!(state.role, RaftRole::Follower);
        assert_eq!(state.current_term, 5);
        assert!(state.voted_for.is_none());
    }

    #[test]
    fn test_voting() {
        let mut state = RaftState::new();

        assert!(state.can_vote_for("node1"));
        state.vote_for("node1".to_string());

        assert!(!state.can_vote_for("node2"));
        assert!(state.can_vote_for("node1"));
    }

    #[test]
    fn test_term_update() {
        let mut state = RaftState::new();
        state.current_term = 3;
        state.role = RaftRole::Leader;

        // Lower term should not update
        assert!(!state.maybe_update_term(2));
        assert_eq!(state.role, RaftRole::Leader);

        // Higher term should update and become follower
        assert!(state.maybe_update_term(5));
        assert_eq!(state.current_term, 5);
        assert_eq!(state.role, RaftRole::Follower);
    }
}
