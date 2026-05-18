# cyberclaw-consensus

Raft-based distributed consensus implementation for the CyberClaw platform.

## Overview

This crate provides a Raft consensus algorithm implementation that enables distributed agreement across multiple nodes in the CyberClaw system. It ensures consistency and fault tolerance for critical system state.

### Raft Algorithm

Raft is a consensus algorithm designed to be understandable. It provides:

- **Leader Election**: Automatic selection of a leader node
- **Log Replication**: Consistent replication of commands across all nodes
- **Safety**: Guarantees that all nodes agree on the same sequence of commands
- **Fault Tolerance**: Continues operating with `(N-1)/2` node failures in an N-node cluster

## Architecture

```
cyberclaw-consensus/
├── consensus.rs       # Generic Consensus trait and RaftConsensus implementation
├── error.rs          # Error types
└── raft/
    ├── node.rs       # Core Raft node implementation
    ├── log.rs        # Replicated log management
    ├── state.rs      # Raft state machine (Follower/Candidate/Leader)
    └── rpc.rs        # gRPC-based RPC implementation
```

## Usage Example

### Creating a Single Node

```rust
use cyberclaw_consensus::{ConsensusBuilder, Consensus};

#[tokio::main]
async fn main() {
    let consensus = ConsensusBuilder::new("node1".to_string())
        .build_raft();

    // Start the consensus node
    consensus.start().await.unwrap();

    // Propose a command (only works if this node is the leader)
    if consensus.is_leader().await {
        let index = consensus.propose(vec![1, 2, 3]).await.unwrap();
        println!("Command committed at index: {}", index);
    }
}
```

### Creating a 3-Node Cluster

```rust
use cyberclaw_consensus::{ConsensusBuilder, PeerInfo, RaftConfig};

#[tokio::main]
async fn main() {
    // Configure node 1
    let node1 = ConsensusBuilder::new("node1".to_string())
        .add_peer("node2".to_string(), "127.0.0.1:9002".to_string())
        .add_peer("node3".to_string(), "127.0.0.1:9003".to_string())
        .with_config(RaftConfig {
            election_timeout_min: 150,
            election_timeout_max: 300,
            heartbeat_interval: 50,
            max_append_entries: 100,
        })
        .build_raft();

    // Start the node
    node1.start().await.unwrap();

    // Wait for leader election
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Check consensus state
    let state = node1.get_state().await;
    println!("Is leader: {}", state.is_leader);
    println!("Current term: {}", state.current_term);
    println!("Leader ID: {:?}", state.leader_id);
}
```

## Configuration

### RaftConfig Parameters

- `election_timeout_min`: Minimum election timeout in milliseconds (default: 150ms)
- `election_timeout_max`: Maximum election timeout in milliseconds (default: 300ms)
- `heartbeat_interval`: Leader heartbeat interval in milliseconds (default: 50ms)
- `max_append_entries`: Maximum entries per AppendEntries RPC (default: 100)

## Cluster Deployment Guide

### Prerequisites

1. Ensure all nodes can communicate via TCP
2. Configure unique node IDs for each instance
3. Set up gRPC endpoints for inter-node communication

### Deployment Steps

1. **Initialize Nodes**: Create each node with its unique ID and peer list
2. **Configure Networking**: Ensure firewall rules allow gRPC traffic
3. **Start Nodes**: Start all nodes within the election timeout window
4. **Monitor Election**: One node will be elected as leader
5. **Verify Cluster**: Check that all nodes recognize the same leader

### Example 3-Node Configuration

```yaml
# Node 1
node_id: "node1"
listen_address: "0.0.0.0:9001"
peers:
  - id: "node2"
    address: "node2.cluster:9002"
  - id: "node3"
    address: "node3.cluster:9003"

# Node 2
node_id: "node2"
listen_address: "0.0.0.0:9002"
peers:
  - id: "node1"
    address: "node1.cluster:9001"
  - id: "node3"
    address: "node3.cluster:9003"

# Node 3
node_id: "node3"
listen_address: "0.0.0.0:9003"
peers:
  - id: "node1"
    address: "node1.cluster:9001"
  - id: "node2"
    address: "node2.cluster:9002"
```

## Fault Recovery

### Node Failure

When a follower node fails:
- The cluster continues operating normally
- The failed node can rejoin and catch up via log replication

When the leader fails:
- Followers detect the absence of heartbeats
- A new election is triggered after the election timeout
- A new leader is elected from the remaining nodes

### Network Partition

In case of network partition:
- The partition with majority nodes elects a leader
- The minority partition cannot make progress
- When the partition heals, nodes reconcile their logs

### Log Compaction

The implementation supports log compaction via snapshots:
- Snapshots capture the state at a specific index
- Old log entries before the snapshot are discarded
- New nodes can catch up quickly using snapshots

## Testing

Run unit tests:
```bash
cargo test
```

Run integration tests:
```bash
cargo test --test cluster_test
```

Run with logging:
```bash
RUST_LOG=debug cargo test
```

## Performance Considerations

- **Batch Processing**: Commands are batched in AppendEntries RPCs
- **Pipelining**: Multiple AppendEntries can be in flight
- **Async I/O**: All operations are async using Tokio
- **Log Compaction**: Prevents unbounded log growth

## Security Notes

- Currently, no authentication between nodes (TODO)
- No encryption of RPC traffic (TODO: TLS support)
- Commands are opaque byte arrays - application responsible for validation

## Future Enhancements

- [ ] TLS support for secure communication
- [ ] Node authentication and authorization
- [ ] Dynamic cluster membership changes
- [ ] Optimized snapshot transfer
- [ ] Metrics and monitoring integration
- [ ] Persistent storage backend options

## License

Apache-2.0