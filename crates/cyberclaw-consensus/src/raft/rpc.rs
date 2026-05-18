use crate::error::{ConsensusError, Result};
use crate::raft::{log::LogEntry, node::RaftNode, state::NodeId};
use std::sync::Arc;
use tonic::{Request, Response, Status};

// Include the generated proto code
pub mod raft_proto {
    tonic::include_proto!("raft");
}

use raft_proto::{
    raft_service_server::{RaftService, RaftServiceServer},
    AppendRequest, AppendResponse, SnapshotRequest, SnapshotResponse, VoteRequest, VoteResponse,
};

pub struct RaftRpcService {
    node: Arc<RaftNode>,
}

impl RaftRpcService {
    pub fn new(node: Arc<RaftNode>) -> Self {
        Self { node }
    }

    pub fn into_service(self) -> RaftServiceServer<Self> {
        RaftServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl RaftService for RaftRpcService {
    async fn request_vote(
        &self,
        request: Request<VoteRequest>,
    ) -> std::result::Result<Response<VoteResponse>, Status> {
        let req = request.into_inner();
        let mut state = self.node.state.write().await;
        let log = self.node.log.read().await;

        // Update term if necessary
        if req.term > state.current_term {
            state.become_follower(req.term);
        }

        let mut vote_granted = false;

        // Check if we can vote for this candidate
        if req.term >= state.current_term && state.can_vote_for(&req.candidate_id) {
            // Check log completeness
            let last_log_term = log.last_term();
            let last_log_index = log.last_index();

            let log_is_current = req.last_log_term > last_log_term
                || (req.last_log_term == last_log_term && req.last_log_index >= last_log_index);

            if log_is_current {
                state.vote_for(req.candidate_id);
                vote_granted = true;
                tracing::info!(
                    "Voted for {} in term {}",
                    state.voted_for.as_ref().unwrap(),
                    state.current_term
                );
            }
        }

        let response = VoteResponse {
            term: state.current_term,
            vote_granted,
        };

        Ok(Response::new(response))
    }

    async fn append_entries(
        &self,
        request: Request<AppendRequest>,
    ) -> std::result::Result<Response<AppendResponse>, Status> {
        let req = request.into_inner();
        let mut state = self.node.state.write().await;
        let mut log = self.node.log.write().await;

        // Update term if necessary
        if req.term > state.current_term {
            state.become_follower(req.term);
        }

        // Reject if term is outdated
        if req.term < state.current_term {
            return Ok(Response::new(AppendResponse {
                term: state.current_term,
                success: false,
                match_index: 0,
            }));
        }

        // Accept current leader for this term and ensure we are follower.
        if state.role != crate::raft::state::RaftRole::Follower {
            state.become_follower(req.term);
        }
        state.leader_id = Some(req.leader_id.clone());

        // Check log consistency
        if req.prev_log_index > 0 && !log.contains(req.prev_log_index, req.prev_log_term) {
            tracing::debug!(
                "Log consistency check failed at index {}",
                req.prev_log_index
            );
            return Ok(Response::new(AppendResponse {
                term: state.current_term,
                success: false,
                match_index: log.last_index(),
            }));
        }

        // Append new entries
        let entries: Vec<LogEntry> = req
            .entries
            .into_iter()
            .map(|e| LogEntry::new(e.index, e.term, e.command))
            .collect();

        if !entries.is_empty() {
            log.append_entries(entries);
        }

        // Update commit index
        if req.leader_commit > log.commit_index() {
            let new_commit = std::cmp::min(req.leader_commit, log.last_index());
            log.update_commit_index(new_commit);
        }

        let response = AppendResponse {
            term: state.current_term,
            success: true,
            match_index: log.last_index(),
        };

        // Signal follower loop that heartbeat / append has been received.
        self.node.mark_heartbeat_received();

        Ok(Response::new(response))
    }

    async fn install_snapshot(
        &self,
        request: Request<SnapshotRequest>,
    ) -> std::result::Result<Response<SnapshotResponse>, Status> {
        let req = request.into_inner();
        let mut state = self.node.state.write().await;

        // Update term if necessary
        if req.term > state.current_term {
            state.become_follower(req.term);
        }

        // Reject if term is outdated
        if req.term < state.current_term {
            return Ok(Response::new(SnapshotResponse {
                term: state.current_term,
            }));
        }

        // Update leader
        state.leader_id = Some(req.leader_id);

        if req.done {
            // Install the complete snapshot
            let mut log = self.node.log.write().await;
            log.install_snapshot(req.last_included_index, req.last_included_term);
            tracing::info!("Installed snapshot up to index {}", req.last_included_index);
        }

        Ok(Response::new(SnapshotResponse {
            term: state.current_term,
        }))
    }
}

pub struct RaftRpcClient {
    client: raft_proto::raft_service_client::RaftServiceClient<tonic::transport::Channel>,
}

impl RaftRpcClient {
    pub async fn connect(addr: String) -> Result<Self> {
        let client = raft_proto::raft_service_client::RaftServiceClient::connect(addr)
            .await
            .map_err(|e| ConsensusError::NetworkError(e.to_string()))?;

        Ok(Self { client })
    }

    pub async fn request_vote(
        &mut self,
        term: u64,
        candidate_id: NodeId,
        last_log_index: u64,
        last_log_term: u64,
    ) -> Result<(u64, bool)> {
        let request = VoteRequest {
            term,
            candidate_id,
            last_log_index,
            last_log_term,
        };

        let response = self
            .client
            .request_vote(request)
            .await
            .map_err(|e| ConsensusError::NetworkError(e.to_string()))?;

        let resp = response.into_inner();
        Ok((resp.term, resp.vote_granted))
    }

    pub async fn append_entries(
        &mut self,
        term: u64,
        leader_id: NodeId,
        prev_log_index: u64,
        prev_log_term: u64,
        entries: Vec<LogEntry>,
        leader_commit: u64,
    ) -> Result<(u64, bool, u64)> {
        let request = AppendRequest {
            term,
            leader_id,
            prev_log_index,
            prev_log_term,
            entries: entries
                .into_iter()
                .map(|e| raft_proto::LogEntry {
                    index: e.index,
                    term: e.term,
                    command: e.command,
                })
                .collect(),
            leader_commit,
        };

        let response = self
            .client
            .append_entries(request)
            .await
            .map_err(|e| ConsensusError::NetworkError(e.to_string()))?;

        let resp = response.into_inner();
        Ok((resp.term, resp.success, resp.match_index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proto_conversion() {
        let log_entry = LogEntry::new(1, 1, vec![1, 2, 3]);
        let proto_entry = raft_proto::LogEntry {
            index: log_entry.index,
            term: log_entry.term,
            command: log_entry.command.clone(),
        };

        assert_eq!(proto_entry.index, 1);
        assert_eq!(proto_entry.term, 1);
        assert_eq!(proto_entry.command, vec![1, 2, 3]);
    }
}
