use crate::error::{ConsensusError, Result};
use crate::raft::{
    log::RaftLog,
    rpc::RaftRpcClient,
    state::{NodeId, RaftRole, RaftState},
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch, Notify, RwLock};
use tokio::time::{interval, sleep, timeout};
use tracing::{debug, error, info};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: NodeId,
    pub address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaftConfig {
    pub election_timeout_min: u64, // ms
    pub election_timeout_max: u64, // ms
    pub heartbeat_interval: u64,   // ms
    pub max_append_entries: usize,
}

impl Default for RaftConfig {
    fn default() -> Self {
        Self {
            election_timeout_min: 150,
            election_timeout_max: 300,
            heartbeat_interval: 50,
            max_append_entries: 100,
        }
    }
}

pub struct RaftNode {
    pub id: NodeId,
    pub state: Arc<RwLock<RaftState>>,
    pub log: Arc<RwLock<RaftLog>>,
    pub peers: HashMap<NodeId, PeerInfo>,
    pub config: RaftConfig,

    // Leader state
    next_index: Arc<RwLock<HashMap<NodeId, u64>>>,
    match_index: Arc<RwLock<HashMap<NodeId, u64>>>,

    // Channels for proposals
    proposal_tx: mpsc::UnboundedSender<ProposalRequest>,
    proposal_rx: Arc<RwLock<mpsc::UnboundedReceiver<ProposalRequest>>>,

    // Shutdown signal — watch::Sender so each spawned loop can subscribe()
    // independently and observe the latest value (sticky after send(true))
    shutdown_tx: watch::Sender<bool>,

    // Heartbeat notification (updated by AppendEntries RPC)
    heartbeat_seq: Arc<AtomicU64>,
    heartbeat_notify: Arc<Notify>,
}

struct ProposalRequest {
    command: Vec<u8>,
    response: oneshot::Sender<Result<u64>>,
}

impl RaftNode {
    pub fn new(id: NodeId, peers: Vec<PeerInfo>, config: RaftConfig) -> Self {
        let peer_map: HashMap<NodeId, PeerInfo> =
            peers.into_iter().map(|p| (p.id.clone(), p)).collect();

        let (proposal_tx, proposal_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, _) = watch::channel(false);

        Self {
            id: id.clone(),
            state: Arc::new(RwLock::new(RaftState::new())),
            log: Arc::new(RwLock::new(RaftLog::new())),
            peers: peer_map,
            config,
            next_index: Arc::new(RwLock::new(HashMap::new())),
            match_index: Arc::new(RwLock::new(HashMap::new())),
            proposal_tx,
            proposal_rx: Arc::new(RwLock::new(proposal_rx)),
            shutdown_tx,
            heartbeat_seq: Arc::new(AtomicU64::new(0)),
            heartbeat_notify: Arc::new(Notify::new()),
        }
    }

    pub async fn start(&self) -> Result<()> {
        info!("Starting Raft node: {}", self.id);

        // Start main loop
        let node = self.clone_for_task();
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        tokio::spawn(async move {
            if let Err(e) = node.run_main_loop(&mut shutdown_rx).await {
                error!("Raft main loop error: {}", e);
            }
        });

        // Start heartbeat loop for leaders
        let node = self.clone_for_task();
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        tokio::spawn(async move {
            node.run_heartbeat_loop(&mut shutdown_rx).await;
        });

        Ok(())
    }

    pub async fn propose(&self, command: Vec<u8>) -> Result<u64> {
        // Check if we are the leader
        let state = self.state.read().await;
        if state.role != RaftRole::Leader {
            return Err(ConsensusError::NotLeader);
        }
        drop(state);

        let (tx, rx) = oneshot::channel();
        let request = ProposalRequest {
            command,
            response: tx,
        };

        self.proposal_tx
            .send(request)
            .map_err(|_| ConsensusError::Internal("Failed to send proposal".to_string()))?;

        rx.await.map_err(|_| {
            ConsensusError::Internal("Failed to receive proposal response".to_string())
        })?
    }

    async fn run_main_loop(&self, shutdown_rx: &mut watch::Receiver<bool>) -> Result<()> {
        loop {
            if *shutdown_rx.borrow() {
                info!("Raft main loop received shutdown signal: {}", self.id);
                return Ok(());
            }

            let role = {
                let state = self.state.read().await;
                state.role
            };

            let step = async {
                match role {
                    RaftRole::Follower => self.run_follower().await,
                    RaftRole::Candidate => self.run_candidate().await,
                    RaftRole::Leader => self.run_leader().await,
                }
            };

            tokio::select! {
                _ = shutdown_rx.changed() => {
                    info!("Raft main loop received shutdown signal: {}", self.id);
                    return Ok(());
                }
                res = step => {
                    res?;
                }
            }
        }
    }

    async fn run_follower(&self) -> Result<()> {
        let election_timeout = self.random_election_timeout();
        debug!("Follower waiting for {} ms", election_timeout.as_millis());

        // Wait for election timeout or heartbeat
        if timeout(election_timeout, self.wait_for_heartbeat())
            .await
            .is_err()
        {
            // Timeout - become candidate
            info!("Election timeout, becoming candidate");
            let mut state = self.state.write().await;
            state.become_candidate();
        }

        Ok(())
    }

    async fn run_candidate(&self) -> Result<()> {
        info!("Running election as candidate");

        // Vote for self
        let mut state = self.state.write().await;
        state.vote_for(self.id.clone());
        state.receive_vote(); // Self vote
        let current_term = state.current_term;
        let total_nodes = self.peers.len() + 1;
        let votes_needed = (total_nodes / 2) + 1;
        drop(state);

        // Request votes from peers
        let vote_results = self.request_votes(current_term).await;

        // Count votes
        let mut state = self.state.write().await;
        if state.role != RaftRole::Candidate || state.current_term != current_term {
            // Role changed or term updated
            return Ok(());
        }

        let votes = vote_results.iter().filter(|v| **v).count() + 1; // +1 for self
        if votes >= votes_needed {
            info!("Won election with {} votes", votes);
            state.become_leader();
            drop(state);
            self.initialize_leader_state().await;
        } else {
            // Wait for election timeout
            let election_timeout = self.random_election_timeout();
            sleep(election_timeout).await;
        }

        Ok(())
    }

    async fn run_leader(&self) -> Result<()> {
        debug!("Running as leader");

        // Process proposals
        let mut proposal_rx = self.proposal_rx.write().await;
        while let Ok(request) = proposal_rx.try_recv() {
            let result = self.process_proposal(request.command).await;
            let _ = request.response.send(result);
        }

        // Short sleep to prevent busy loop
        sleep(Duration::from_millis(10)).await;
        Ok(())
    }

    async fn run_heartbeat_loop(&self, shutdown_rx: &mut watch::Receiver<bool>) {
        let mut heartbeat_interval =
            interval(Duration::from_millis(self.config.heartbeat_interval));

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    info!("Raft heartbeat loop received shutdown signal: {}", self.id);
                    return;
                }
                _ = heartbeat_interval.tick() => {
                    let state = self.state.read().await;
                    if state.role == RaftRole::Leader {
                        drop(state);
                        self.send_heartbeats().await;
                    }
                }
            }
        }
    }

    async fn process_proposal(&self, command: Vec<u8>) -> Result<u64> {
        let mut log = self.log.write().await;
        let state = self.state.read().await;

        let index = log.last_index() + 1;
        let entry = crate::raft::log::LogEntry::new(index, state.current_term, command);

        log.append(entry);
        drop(log);
        drop(state);

        // Replicate to followers
        self.replicate_log().await;

        Ok(index)
    }

    async fn send_heartbeats(&self) {
        debug!("Sending heartbeats");

        for peer_id in self.peers.keys() {
            let node = self.clone_for_task();
            let peer_id = peer_id.clone();
            tokio::spawn(async move {
                node.send_append_entries(&peer_id).await;
            });
        }
    }

    async fn replicate_log(&self) {
        for peer_id in self.peers.keys() {
            let node = self.clone_for_task();
            let peer_id = peer_id.clone();
            tokio::spawn(async move {
                node.send_append_entries(&peer_id).await;
            });
        }
    }

    async fn send_append_entries(&self, peer_id: &NodeId) {
        let peer = match self.peers.get(peer_id) {
            Some(peer) => peer,
            None => {
                error!("Unknown peer: {}", peer_id);
                return;
            }
        };

        let (current_term, is_leader) = {
            let state = self.state.read().await;
            (state.current_term, state.role == RaftRole::Leader)
        };
        if !is_leader {
            return;
        }

        let (next_idx, prev_log_index, prev_log_term, entries, leader_commit) = {
            let next_idx = {
                let next = self.next_index.read().await;
                next.get(peer_id).copied().unwrap_or(1)
            };
            let log = self.log.read().await;
            let mut entries = log.get_from(next_idx);
            if entries.len() > self.config.max_append_entries {
                entries.truncate(self.config.max_append_entries);
            }
            let prev_log_index = next_idx.saturating_sub(1);
            let prev_log_term = if prev_log_index == 0 {
                0
            } else {
                log.term_at(prev_log_index).unwrap_or(0)
            };
            (
                next_idx,
                prev_log_index,
                prev_log_term,
                entries,
                log.commit_index(),
            )
        };

        let peer_addr = Self::normalize_peer_addr(&peer.address);
        let mut client = match RaftRpcClient::connect(peer_addr.clone()).await {
            Ok(client) => client,
            Err(err) => {
                debug!(
                    "Failed to connect to peer {} at {}: {}",
                    peer_id, peer_addr, err
                );
                return;
            }
        };

        match client
            .append_entries(
                current_term,
                self.id.clone(),
                prev_log_index,
                prev_log_term,
                entries,
                leader_commit,
            )
            .await
        {
            Ok((peer_term, success, peer_match_index)) => {
                if peer_term > current_term {
                    let mut state = self.state.write().await;
                    state.become_follower(peer_term);
                    state.leader_id = None;
                    return;
                }

                if success {
                    let mut match_index = self.match_index.write().await;
                    match_index.insert(peer_id.clone(), peer_match_index);
                    drop(match_index);

                    let mut next_index = self.next_index.write().await;
                    next_index.insert(peer_id.clone(), peer_match_index.saturating_add(1));
                } else {
                    let mut next_index = self.next_index.write().await;
                    let current = next_index.get(peer_id).copied().unwrap_or(next_idx);
                    let reduced = current.saturating_sub(1).max(1);
                    next_index.insert(peer_id.clone(), reduced);
                }
            }
            Err(err) => {
                debug!("AppendEntries RPC to {} failed: {}", peer_id, err);
            }
        }
    }

    async fn request_votes(&self, term: u64) -> Vec<bool> {
        let mut votes = Vec::new();
        let (last_log_index, last_log_term) = {
            let log = self.log.read().await;
            (log.last_index(), log.last_term())
        };

        for peer in self.peers.values() {
            let peer_addr = Self::normalize_peer_addr(&peer.address);
            let mut client = match RaftRpcClient::connect(peer_addr.clone()).await {
                Ok(client) => client,
                Err(err) => {
                    debug!(
                        "Failed to connect to peer {} at {} for RequestVote: {}",
                        peer.id, peer_addr, err
                    );
                    votes.push(false);
                    continue;
                }
            };

            match client
                .request_vote(term, self.id.clone(), last_log_index, last_log_term)
                .await
            {
                Ok((peer_term, vote_granted)) => {
                    if peer_term > term {
                        let mut state = self.state.write().await;
                        state.become_follower(peer_term);
                        state.leader_id = None;
                        return vec![false; self.peers.len()];
                    }
                    votes.push(vote_granted);
                }
                Err(err) => {
                    debug!("RequestVote RPC to {} failed: {}", peer.id, err);
                    votes.push(false);
                }
            }
        }

        votes
    }

    async fn wait_for_heartbeat(&self) {
        let observed = self.heartbeat_seq.load(Ordering::SeqCst);
        loop {
            self.heartbeat_notify.notified().await;
            if self.heartbeat_seq.load(Ordering::SeqCst) != observed {
                return;
            }
        }
    }

    pub(crate) fn mark_heartbeat_received(&self) {
        self.heartbeat_seq.fetch_add(1, Ordering::SeqCst);
        self.heartbeat_notify.notify_waiters();
    }

    fn normalize_peer_addr(addr: &str) -> String {
        if addr.starts_with("http://") || addr.starts_with("https://") {
            addr.to_string()
        } else {
            format!("http://{}", addr)
        }
    }

    async fn initialize_leader_state(&self) {
        let log = self.log.read().await;
        let last_index = log.last_index();
        drop(log);

        let mut next_index = self.next_index.write().await;
        let mut match_index = self.match_index.write().await;

        for peer_id in self.peers.keys() {
            next_index.insert(peer_id.clone(), last_index + 1);
            match_index.insert(peer_id.clone(), 0);
        }
    }

    fn random_election_timeout(&self) -> Duration {
        let mut rng = rand::thread_rng();
        let timeout_ms =
            rng.gen_range(self.config.election_timeout_min..=self.config.election_timeout_max);
        Duration::from_millis(timeout_ms)
    }

    fn clone_for_task(&self) -> Arc<Self> {
        // This is a simplified version - in real implementation, we'd use Arc<Self>
        Arc::new(Self {
            id: self.id.clone(),
            state: self.state.clone(),
            log: self.log.clone(),
            peers: self.peers.clone(),
            config: self.config.clone(),
            next_index: self.next_index.clone(),
            match_index: self.match_index.clone(),
            proposal_tx: self.proposal_tx.clone(),
            proposal_rx: self.proposal_rx.clone(),
            shutdown_tx: self.shutdown_tx.clone(),
            heartbeat_seq: self.heartbeat_seq.clone(),
            heartbeat_notify: self.heartbeat_notify.clone(),
        })
    }

    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down Raft node: {}", self.id);
        self.shutdown_tx
            .send(true)
            .map_err(|_| ConsensusError::Internal("No active shutdown listeners".to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_node_creation() {
        let peers = vec![
            PeerInfo {
                id: "node2".to_string(),
                address: "127.0.0.1:9002".to_string(),
            },
            PeerInfo {
                id: "node3".to_string(),
                address: "127.0.0.1:9003".to_string(),
            },
        ];

        let node = RaftNode::new("node1".to_string(), peers, RaftConfig::default());

        assert_eq!(node.id, "node1");
        assert_eq!(node.peers.len(), 2);

        let state = node.state.read().await;
        assert_eq!(state.role, RaftRole::Follower);
        assert_eq!(state.current_term, 0);
    }

    #[tokio::test]
    async fn test_propose_not_leader() {
        let node = RaftNode::new("node1".to_string(), vec![], RaftConfig::default());

        let result = node.propose(vec![1, 2, 3]).await;
        assert!(matches!(result, Err(ConsensusError::NotLeader)));
    }

    #[tokio::test]
    async fn test_shutdown_signals_active_loops() {
        // Regression: previously `RaftNode::new` did `(shutdown_tx, _) = mpsc::channel(1)`,
        // dropping the receiver immediately. `shutdown()` would silently fail because the
        // send target was already gone. Now `start()` creates fresh receivers via
        // `watch::Sender::subscribe()`, so shutdown actually reaches the spawned loops.
        let node = RaftNode::new("node1".to_string(), vec![], RaftConfig::default());
        assert_eq!(node.shutdown_tx.receiver_count(), 0);

        node.start().await.expect("start should succeed");
        // Two loops, two subscribers.
        assert_eq!(node.shutdown_tx.receiver_count(), 2);

        node.shutdown()
            .await
            .expect("shutdown send should reach active subscribers");

        // Both loops observe the change and exit; receiver_count drains.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while node.shutdown_tx.receiver_count() > 0 && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            node.shutdown_tx.receiver_count(),
            0,
            "spawned loops did not exit after shutdown signal",
        );
    }
}
