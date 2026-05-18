use chrono::Utc;
use cyberclaw_core::cluster::{CapabilityPlacement, NodeRecord, PlacementDecision};
use cyberclaw_core::ids::ExecutionId;

/// Placement engine trait for node selection (Milestone C)
pub trait PlacementEngine: Send + Sync {
    /// Select a node for execution placement
    ///
    /// Rules:
    /// - Only select active + healthy nodes
    /// - Satisfy labels/runtime/network_zone requirements
    /// - Prefer node with minimum load (current_executions)
    fn place(
        &self,
        execution_id: ExecutionId,
        placement: &CapabilityPlacement,
        active_nodes: &[NodeRecord],
    ) -> anyhow::Result<PlacementDecision>;
}

/// In-memory implementation of PlacementEngine
#[derive(Default)]
pub struct InMemoryPlacementEngine;

impl InMemoryPlacementEngine {
    pub fn new() -> Self {
        Self
    }

    /// Check if node satisfies label requirements
    fn matches_labels(node: &NodeRecord, required_labels: &[String]) -> bool {
        if required_labels.is_empty() {
            return true;
        }
        required_labels
            .iter()
            .all(|label| node.labels.contains(label))
    }

    /// Extract runtime labels from node metadata.
    ///
    /// Supported label formats:
    /// - `runtime:native`
    /// - `runtime:process`
    /// - `runtime:container`
    /// - direct labels: `native|process|container|remote`
    fn extract_node_runtimes(node: &NodeRecord) -> Vec<String> {
        node.labels
            .iter()
            .filter_map(|label| {
                if let Some(runtime) = label.strip_prefix("runtime:") {
                    return Some(runtime.to_ascii_lowercase());
                }
                match label.to_ascii_lowercase().as_str() {
                    "native" | "process" | "container" | "remote" => {
                        Some(label.to_ascii_lowercase())
                    }
                    _ => None,
                }
            })
            .collect()
    }

    /// Check if node satisfies runtime requirements.
    ///
    /// Compatibility rule:
    /// - If node has no explicit runtime labels, it is treated as `native` only.
    fn matches_runtime(node: &NodeRecord, required_runtime: &[String]) -> bool {
        if required_runtime.is_empty() {
            return true;
        }

        let required: Vec<String> = required_runtime
            .iter()
            .map(|item| item.to_ascii_lowercase())
            .collect();

        let node_runtimes = Self::extract_node_runtimes(node);
        if node_runtimes.is_empty() {
            return required.iter().all(|item| item == "native");
        }

        required
            .iter()
            .all(|item| node_runtimes.iter().any(|runtime| runtime == item))
    }

    /// Check if node satisfies network zone requirements
    fn matches_network_zone(node: &NodeRecord, required_zone: Option<&String>) -> bool {
        match required_zone {
            None => true,
            Some(required) => node.zone.as_ref() == Some(required),
        }
    }
}

impl PlacementEngine for InMemoryPlacementEngine {
    fn place(
        &self,
        execution_id: ExecutionId,
        placement: &CapabilityPlacement,
        active_nodes: &[NodeRecord],
    ) -> anyhow::Result<PlacementDecision> {
        if active_nodes.is_empty() {
            anyhow::bail!("no active nodes available for placement");
        }

        // Filter nodes by requirements
        let mut candidates: Vec<&NodeRecord> = active_nodes
            .iter()
            .filter(|node| Self::matches_labels(node, &placement.allowed_node_labels))
            .filter(|node| Self::matches_runtime(node, &placement.required_runtime))
            .filter(|node| Self::matches_network_zone(node, placement.network_zone.as_ref()))
            .collect();

        if candidates.is_empty() {
            anyhow::bail!(
                "no nodes satisfy placement requirements: labels={:?}, runtime={:?}, zone={:?}",
                placement.allowed_node_labels,
                placement.required_runtime,
                placement.network_zone
            );
        }

        // Sort by current load (ascending)
        candidates.sort_by_key(|node| node.current_executions);

        // Select node with minimum load
        let selected = candidates[0];

        let decision = PlacementDecision {
            execution_id,
            scheduled_node_id: selected.id.clone(),
            reason: format!(
                "selected node {} with {} current executions (min load)",
                selected.id, selected.current_executions
            ),
            decided_at: Utc::now(),
        };

        Ok(decision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::cluster::{MembershipState, NodeCapacity, NodeHealth, NodeRole};
    use cyberclaw_core::ids::NodeId;

    fn create_test_node(id: &str, current_executions: u32, labels: Vec<String>) -> NodeRecord {
        NodeRecord {
            id: NodeId::from_string(id.to_string()).unwrap(),
            role: NodeRole::Worker,
            labels,
            region: Some("us-east-1".to_string()),
            zone: Some("us-east-1a".to_string()),
            health: NodeHealth::Healthy,
            membership_state: MembershipState::Active,
            capacity: NodeCapacity {
                max_executions: Some(10),
                max_cpu_millis: None,
                max_memory_mb: None,
            },
            current_executions,
            last_heartbeat_at: Utc::now(),
        }
    }

    #[test]
    fn test_place_selects_min_load() {
        let engine = InMemoryPlacementEngine::new();
        let execution_id = ExecutionId::new();

        let nodes = vec![
            create_test_node("node-1", 5, vec!["worker".to_string()]),
            create_test_node("node-2", 2, vec!["worker".to_string()]),
            create_test_node("node-3", 8, vec!["worker".to_string()]),
        ];

        let placement = CapabilityPlacement::default();

        let decision = engine
            .place(execution_id.clone(), &placement, &nodes)
            .unwrap();

        assert_eq!(
            decision.scheduled_node_id,
            NodeId::from_string("node-2".to_string()).unwrap()
        );
        assert_eq!(decision.execution_id, execution_id);
    }

    #[test]
    fn test_place_filters_by_labels() {
        let engine = InMemoryPlacementEngine::new();
        let execution_id = ExecutionId::new();

        let nodes = vec![
            create_test_node("node-1", 1, vec!["worker".to_string()]),
            create_test_node("node-2", 1, vec!["gpu".to_string()]),
            create_test_node("node-3", 1, vec!["worker".to_string(), "gpu".to_string()]),
        ];

        let placement = CapabilityPlacement {
            allowed_node_labels: vec!["gpu".to_string()],
            ..Default::default()
        };

        let decision = engine.place(execution_id, &placement, &nodes).unwrap();

        // Should select node-2 or node-3 (both have gpu label, same load)
        assert!(
            decision.scheduled_node_id == NodeId::from_string("node-2".to_string()).unwrap()
                || decision.scheduled_node_id == NodeId::from_string("node-3".to_string()).unwrap()
        );
    }

    #[test]
    fn test_place_fails_no_nodes() {
        let engine = InMemoryPlacementEngine::new();
        let execution_id = ExecutionId::new();

        let nodes = vec![];
        let placement = CapabilityPlacement::default();

        let result = engine.place(execution_id, &placement, &nodes);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no active nodes available"));
    }

    #[test]
    fn test_place_fails_no_matching_labels() {
        let engine = InMemoryPlacementEngine::new();
        let execution_id = ExecutionId::new();

        let nodes = vec![
            create_test_node("node-1", 1, vec!["worker".to_string()]),
            create_test_node("node-2", 1, vec!["cpu".to_string()]),
        ];

        let placement = CapabilityPlacement {
            allowed_node_labels: vec!["gpu".to_string()],
            ..Default::default()
        };

        let result = engine.place(execution_id, &placement, &nodes);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no nodes satisfy placement requirements"));
    }

    #[test]
    fn test_place_runtime_mismatch_fails() {
        let engine = InMemoryPlacementEngine::new();
        let execution_id = ExecutionId::new();

        let nodes = vec![
            create_test_node("node-1", 1, vec!["worker".to_string()]),
            create_test_node(
                "node-2",
                1,
                vec!["worker".to_string(), "runtime:native".to_string()],
            ),
        ];

        let placement = CapabilityPlacement {
            required_runtime: vec!["process".to_string()],
            ..Default::default()
        };

        let result = engine.place(execution_id, &placement, &nodes);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no nodes satisfy placement requirements"));
    }

    #[test]
    fn test_place_runtime_match_succeeds() {
        let engine = InMemoryPlacementEngine::new();
        let execution_id = ExecutionId::new();

        let nodes = vec![
            create_test_node(
                "node-1",
                1,
                vec!["worker".to_string(), "runtime:process".to_string()],
            ),
            create_test_node(
                "node-2",
                0,
                vec!["worker".to_string(), "runtime:native".to_string()],
            ),
        ];

        let placement = CapabilityPlacement {
            required_runtime: vec!["process".to_string()],
            ..Default::default()
        };

        let decision = engine.place(execution_id, &placement, &nodes).unwrap();
        assert_eq!(
            decision.scheduled_node_id,
            NodeId::from_string("node-1".to_string()).unwrap()
        );
    }
}
