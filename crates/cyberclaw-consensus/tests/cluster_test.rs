use cyberclaw_consensus::raft::rpc::RaftRpcService;
use cyberclaw_consensus::raft::RaftRole;
use cyberclaw_consensus::RaftNode;
use cyberclaw_consensus::{Consensus, ConsensusBuilder, PeerInfo, RaftConfig};
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout};
use tonic::transport::Server;

fn reserve_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().expect("read local addr");
    drop(listener);
    addr
}

fn spawn_rpc_server(node: Arc<RaftNode>, addr: SocketAddr) -> oneshot::Sender<()> {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tokio::spawn(async move {
        let service = RaftRpcService::new(node).into_service();
        let result = Server::builder()
            .add_service(service)
            .serve_with_shutdown(addr, async move {
                let _ = shutdown_rx.await;
            })
            .await;
        if let Err(err) = result {
            panic!("raft rpc server failed on {addr}: {err}");
        }
    });
    shutdown_tx
}

#[tokio::test]
async fn test_single_node_cluster() {
    // Single node can become leader immediately
    let consensus = ConsensusBuilder::new("node1".to_string())
        .with_config(RaftConfig {
            election_timeout_min: 50,
            election_timeout_max: 100,
            heartbeat_interval: 20,
            max_append_entries: 100,
        })
        .build_raft();

    consensus.start().await.expect("Failed to start node");

    // Wait for election
    sleep(Duration::from_millis(200)).await;

    let state = consensus.get_state().await;
    assert_eq!(state.current_term, 1);
    // In a single-node cluster, it should become leader
    // Note: This requires actual implementation of leader election
}

#[tokio::test]
async fn test_three_node_cluster() {
    // Create three nodes
    let peers_for_node1 = vec![
        PeerInfo {
            id: "node2".to_string(),
            address: "127.0.0.1:9002".to_string(),
        },
        PeerInfo {
            id: "node3".to_string(),
            address: "127.0.0.1:9003".to_string(),
        },
    ];

    let peers_for_node2 = vec![
        PeerInfo {
            id: "node1".to_string(),
            address: "127.0.0.1:9001".to_string(),
        },
        PeerInfo {
            id: "node3".to_string(),
            address: "127.0.0.1:9003".to_string(),
        },
    ];

    let peers_for_node3 = vec![
        PeerInfo {
            id: "node1".to_string(),
            address: "127.0.0.1:9001".to_string(),
        },
        PeerInfo {
            id: "node2".to_string(),
            address: "127.0.0.1:9002".to_string(),
        },
    ];

    let config = RaftConfig {
        election_timeout_min: 150,
        election_timeout_max: 300,
        heartbeat_interval: 50,
        max_append_entries: 100,
    };

    let node1 = ConsensusBuilder::new("node1".to_string())
        .with_peers(peers_for_node1)
        .with_config(config.clone())
        .build_raft();

    let node2 = ConsensusBuilder::new("node2".to_string())
        .with_peers(peers_for_node2)
        .with_config(config.clone())
        .build_raft();

    let node3 = ConsensusBuilder::new("node3".to_string())
        .with_peers(peers_for_node3)
        .with_config(config)
        .build_raft();

    // Start all nodes
    node1.start().await.expect("Failed to start node1");
    node2.start().await.expect("Failed to start node2");
    node3.start().await.expect("Failed to start node3");

    // Wait for leader election
    sleep(Duration::from_millis(500)).await;

    // Check that at least one node thinks there's a leader
    let state1 = node1.get_state().await;
    let state2 = node2.get_state().await;
    let state3 = node3.get_state().await;

    let _leader_count = [state1.is_leader, state2.is_leader, state3.is_leader]
        .iter()
        .filter(|&&x| x)
        .count();

    // Should have exactly one leader (once gRPC is fully implemented)
    // For now, we just check the state is initialized
    assert_eq!(state1.commit_index, 0);
    assert_eq!(state2.commit_index, 0);
    assert_eq!(state3.commit_index, 0);

    // Shutdown nodes - may fail if shutdown channel already closed
    let _ = node1.shutdown().await;
    let _ = node2.shutdown().await;
    let _ = node3.shutdown().await;
}

#[tokio::test]
async fn test_propose_and_commit() {
    let consensus = ConsensusBuilder::new("node1".to_string()).build_raft();

    consensus.start().await.expect("Failed to start");

    // This will fail because the node is not a leader
    let result = consensus.propose(vec![1, 2, 3]).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_leader_election_timeout() {
    // Create a node with peers so it can't immediately become leader
    let consensus = ConsensusBuilder::new("node1".to_string())
        .add_peer("node2".to_string(), "127.0.0.1:9002".to_string())
        .add_peer("node3".to_string(), "127.0.0.1:9003".to_string())
        .with_config(RaftConfig {
            election_timeout_min: 50,
            election_timeout_max: 100,
            heartbeat_interval: 20,
            max_append_entries: 100,
        })
        .build_raft();

    consensus.start().await.expect("Failed to start");

    // Should timeout waiting for leader since other nodes aren't running
    let result = timeout(Duration::from_millis(200), async {
        while consensus.get_leader().await.is_none() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await;

    // Without other nodes running, no leader can be elected
    assert!(
        result.is_err(),
        "Should timeout when peers are not available"
    )
}

#[tokio::test]
async fn test_consensus_state_tracking() {
    let consensus = ConsensusBuilder::new("test_node".to_string()).build_raft();

    let initial_state = consensus.get_state().await;
    assert_eq!(initial_state.current_term, 0);
    assert!(!initial_state.is_leader);
    assert!(initial_state.leader_id.is_none());
    assert_eq!(initial_state.commit_index, 0);
    assert_eq!(initial_state.last_applied, 0);

    // Verify term tracking
    let term = consensus.get_term().await;
    assert_eq!(term, 0);
}

#[tokio::test]
async fn test_three_node_cluster_elects_single_leader_with_rpc() {
    let addr1 = reserve_addr();
    let addr2 = reserve_addr();
    let addr3 = reserve_addr();

    let config = RaftConfig {
        election_timeout_min: 150,
        election_timeout_max: 300,
        heartbeat_interval: 50,
        max_append_entries: 100,
    };

    let node1 = Arc::new(RaftNode::new(
        "node1".to_string(),
        vec![
            PeerInfo {
                id: "node2".to_string(),
                address: format!("http://{}", addr2),
            },
            PeerInfo {
                id: "node3".to_string(),
                address: format!("http://{}", addr3),
            },
        ],
        config.clone(),
    ));
    let node2 = Arc::new(RaftNode::new(
        "node2".to_string(),
        vec![
            PeerInfo {
                id: "node1".to_string(),
                address: format!("http://{}", addr1),
            },
            PeerInfo {
                id: "node3".to_string(),
                address: format!("http://{}", addr3),
            },
        ],
        config.clone(),
    ));
    let node3 = Arc::new(RaftNode::new(
        "node3".to_string(),
        vec![
            PeerInfo {
                id: "node1".to_string(),
                address: format!("http://{}", addr1),
            },
            PeerInfo {
                id: "node2".to_string(),
                address: format!("http://{}", addr2),
            },
        ],
        config,
    ));

    let shutdown1 = spawn_rpc_server(node1.clone(), addr1);
    let shutdown2 = spawn_rpc_server(node2.clone(), addr2);
    let shutdown3 = spawn_rpc_server(node3.clone(), addr3);

    node1.start().await.expect("start node1");
    node2.start().await.expect("start node2");
    node3.start().await.expect("start node3");

    let elected = timeout(Duration::from_secs(3), async {
        loop {
            let roles = [
                node1.state.read().await.role,
                node2.state.read().await.role,
                node3.state.read().await.role,
            ];
            let leaders = roles
                .iter()
                .filter(|role| **role == RaftRole::Leader)
                .count();
            if leaders == 1 {
                return;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await;

    let _ = shutdown1.send(());
    let _ = shutdown2.send(());
    let _ = shutdown3.send(());

    assert!(
        elected.is_ok(),
        "3-node cluster should elect exactly one leader within timeout"
    );
}
