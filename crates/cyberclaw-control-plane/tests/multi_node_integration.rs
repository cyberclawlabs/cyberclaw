use chrono::Utc;
/// Integration tests for Milestone C: Multi-node Foundation v1
///
/// Tests the complete workflow of:
/// 1. Multiple worker nodes joining the cluster
/// 2. Execution placement across workers
/// 3. Lease management and expiration
/// 4. Execution reassignment after lease timeout
use cyberclaw_control_plane::{
    InMemoryLeaseManager, InMemoryMembershipService, InMemoryPlacementEngine, LeaseConfig,
    LeaseManager, MembershipConfig, MembershipService, PlacementEngine,
};
use cyberclaw_core::cluster::{
    CapabilityPlacement, MembershipState, NodeCapacity, NodeHealth, NodeRecord, NodeRole,
};
use cyberclaw_core::ids::{ExecutionId, NodeId};

fn create_worker_node(id: &str, labels: Vec<String>) -> NodeRecord {
    NodeRecord {
        id: NodeId::from_string(id.to_string()).unwrap(),
        role: NodeRole::Worker,
        labels,
        region: Some("us-east-1".to_string()),
        zone: Some("us-east-1a".to_string()),
        health: NodeHealth::Healthy,
        membership_state: MembershipState::Joining,
        capacity: NodeCapacity {
            max_executions: Some(10),
            max_cpu_millis: None,
            max_memory_mb: None,
        },
        current_executions: 0,
        last_heartbeat_at: Utc::now(),
    }
}

#[tokio::test]
async fn test_multi_worker_placement_and_reassignment() {
    // Setup: Create services
    let membership = InMemoryMembershipService::default();
    let placement_engine = InMemoryPlacementEngine::new();
    let lease_config = LeaseConfig {
        default_ttl_secs: 1, // Short TTL for testing
    };
    let lease_manager = InMemoryLeaseManager::new(lease_config);

    // Step 1: Join 3 worker nodes
    let node1 = create_worker_node("worker-1", vec!["worker".to_string()]);
    let node2 = create_worker_node("worker-2", vec!["worker".to_string()]);
    let node3 = create_worker_node("worker-3", vec!["worker".to_string()]);

    membership.join(node1.clone()).unwrap();
    membership.join(node2.clone()).unwrap();
    membership.join(node3.clone()).unwrap();

    // Send heartbeats to promote to Active
    membership.heartbeat(&node1.id).unwrap();
    membership.heartbeat(&node2.id).unwrap();
    membership.heartbeat(&node3.id).unwrap();

    // Verify all nodes are active
    let active_nodes = membership.list_active_nodes().unwrap();
    assert_eq!(active_nodes.len(), 3);

    // Step 2: Place execution on a worker
    let execution_id = ExecutionId::new();
    let placement_spec = CapabilityPlacement::default();

    let decision = placement_engine
        .place(execution_id.clone(), &placement_spec, &active_nodes)
        .unwrap();

    println!(
        "Execution {} placed on node {} (reason: {})",
        execution_id, decision.scheduled_node_id, decision.reason
    );

    // Step 3: Acquire lease for the execution
    let lease_id = lease_manager
        .acquire(
            execution_id.clone(),
            decision.scheduled_node_id.clone(),
            None,
        )
        .await
        .unwrap();

    println!("Lease {} acquired for execution {}", lease_id, execution_id);

    // Verify lease is active
    let lease = lease_manager.get_lease(&lease_id).await.unwrap().unwrap();
    assert!(lease.is_active());
    assert_eq!(lease.execution_id, execution_id);
    assert_eq!(lease.owner_node_id, decision.scheduled_node_id);
    assert_eq!(lease.handoff_count, 0);

    // Step 4: Wait for lease to expire
    println!("Waiting for lease to expire (2 seconds)...");
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Step 5: Mark expired leases
    let expired_executions = lease_manager.expire_and_mark().await.unwrap();
    assert_eq!(expired_executions.len(), 1);
    assert_eq!(expired_executions[0], execution_id);

    println!("Lease expired for execution {}", execution_id);

    // Verify lease is now expired
    let expired_lease = lease_manager.get_lease(&lease_id).await.unwrap().unwrap();
    assert!(!expired_lease.is_active());

    // Step 6: Reassign execution to a different node
    let old_node_id = decision.scheduled_node_id.clone();

    // Select a different node (find one that's not the old node)
    let new_node = active_nodes
        .iter()
        .find(|n| n.id != old_node_id)
        .expect("Should have other nodes available");

    let reassignment = lease_manager
        .reassign(&execution_id, new_node.id.clone(), None)
        .await
        .unwrap();

    println!(
        "Execution {} reassigned from {} to {} (handoff count: {})",
        reassignment.execution_id,
        reassignment.old_node_id,
        reassignment.new_node_id,
        reassignment.handoff_count
    );

    // Step 7: Verify reassignment
    assert_eq!(reassignment.execution_id, execution_id);
    assert_eq!(reassignment.old_node_id, old_node_id);
    assert_eq!(reassignment.new_node_id, new_node.id);
    assert_eq!(reassignment.handoff_count, 1);

    // Verify new lease is active
    let new_lease = lease_manager
        .get_lease(&reassignment.new_lease_id)
        .await
        .unwrap()
        .unwrap();
    assert!(new_lease.is_active());
    assert_eq!(new_lease.execution_id, execution_id);
    assert_eq!(new_lease.owner_node_id, new_node.id);
    assert_eq!(new_lease.handoff_count, 1);

    println!("✓ Multi-worker placement and reassignment test passed!");
}

#[tokio::test]
async fn test_node_failure_and_recovery() {
    // Setup
    let membership_config = MembershipConfig {
        heartbeat_timeout_secs: 1,
        suspect_timeout_secs: 2,
    };
    let membership = InMemoryMembershipService::new(membership_config);
    let lease_manager = InMemoryLeaseManager::default();

    // Join node and promote to active
    let node = create_worker_node("worker-1", vec!["worker".to_string()]);
    membership.join(node.clone()).unwrap();
    membership.heartbeat(&node.id).unwrap();

    // Acquire lease
    let execution_id = ExecutionId::new();
    let lease_id = lease_manager
        .acquire(execution_id.clone(), node.id.clone(), None)
        .await
        .unwrap();

    // Simulate node failure (stop sending heartbeats)
    println!("Simulating node failure...");
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Check for timeout
    let evicted = membership.evict_timeout_nodes().unwrap();
    assert_eq!(evicted.len(), 0); // Not evicted yet, just marked as Suspect

    let node_membership = membership.get_membership(&node.id).unwrap().unwrap();
    assert_eq!(node_membership.membership_state, MembershipState::Suspect);

    // Simulate node recovery (send heartbeat)
    println!("Simulating node recovery...");
    membership.heartbeat(&node.id).unwrap();

    // Verify node recovered to Active
    let recovered_membership = membership.get_membership(&node.id).unwrap().unwrap();
    assert_eq!(
        recovered_membership.membership_state,
        MembershipState::Active
    );

    // Verify lease still active (no expiration during recovery)
    let lease = lease_manager.get_lease(&lease_id).await.unwrap().unwrap();
    assert!(lease.is_active());

    println!("✓ Node failure and recovery test passed!");
}

#[test]
fn test_placement_with_labels() {
    // Setup
    let membership = InMemoryMembershipService::default();
    let placement_engine = InMemoryPlacementEngine::new();

    // Join nodes with different labels
    let cpu_node = create_worker_node("cpu-worker", vec!["worker".to_string(), "cpu".to_string()]);
    let gpu_node = create_worker_node("gpu-worker", vec!["worker".to_string(), "gpu".to_string()]);

    membership.join(cpu_node.clone()).unwrap();
    membership.join(gpu_node.clone()).unwrap();
    membership.heartbeat(&cpu_node.id).unwrap();
    membership.heartbeat(&gpu_node.id).unwrap();

    let active_nodes = membership.list_active_nodes().unwrap();

    // Test 1: Placement requiring GPU label
    let execution_id_1 = ExecutionId::new();
    let gpu_placement = CapabilityPlacement {
        allowed_node_labels: vec!["gpu".to_string()],
        ..Default::default()
    };

    let decision_1 = placement_engine
        .place(execution_id_1, &gpu_placement, &active_nodes)
        .unwrap();

    assert_eq!(decision_1.scheduled_node_id, gpu_node.id);
    println!(
        "GPU execution placed on {} (correct)",
        decision_1.scheduled_node_id
    );

    // Test 2: Placement with no label requirements (selects any)
    let execution_id_2 = ExecutionId::new();
    let any_placement = CapabilityPlacement::default();

    let decision_2 = placement_engine
        .place(execution_id_2, &any_placement, &active_nodes)
        .unwrap();

    // Should select one of the nodes
    assert!(
        decision_2.scheduled_node_id == cpu_node.id || decision_2.scheduled_node_id == gpu_node.id
    );
    println!("Any execution placed on {}", decision_2.scheduled_node_id);

    println!("✓ Placement with labels test passed!");
}

#[tokio::test]
async fn test_multiple_execution_reassignments() {
    // Test that handoff_count increments correctly across multiple reassignments
    let lease_config = LeaseConfig {
        default_ttl_secs: 1,
    };
    let lease_manager = InMemoryLeaseManager::new(lease_config);

    let execution_id = ExecutionId::new();
    let node1 = NodeId::from_string("node-1".to_string()).unwrap();
    let node2 = NodeId::from_string("node-2".to_string()).unwrap();
    let node3 = NodeId::from_string("node-3".to_string()).unwrap();

    // Initial lease
    lease_manager
        .acquire(execution_id.clone(), node1.clone(), None)
        .await
        .unwrap();

    // Wait and expire
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    lease_manager.expire_and_mark().await.unwrap();

    // First reassignment
    let reassign1 = lease_manager
        .reassign(&execution_id, node2.clone(), None)
        .await
        .unwrap();
    assert_eq!(reassign1.handoff_count, 1);

    // Wait and expire
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    lease_manager.expire_and_mark().await.unwrap();

    // Second reassignment
    let reassign2 = lease_manager
        .reassign(&execution_id, node3, None)
        .await
        .unwrap();
    assert_eq!(reassign2.handoff_count, 2);

    println!("✓ Multiple execution reassignments test passed!");
}
