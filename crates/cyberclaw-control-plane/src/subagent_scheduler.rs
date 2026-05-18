use cyberclaw_core::prelude::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

// Resource limit constants for subagent scheduling
/// Maximum number of concurrent subagents allowed across the scheduler.
pub const MAX_SUBAGENTS: usize = 100;
/// Maximum memory per individual subagent in megabytes.
pub const MAX_MEMORY_PER_SUBAGENT_MB: usize = 256;
/// Maximum total memory across all active subagents in megabytes.
pub const MAX_TOTAL_MEMORY_MB: usize = 4096;
/// Default timeout for a single subagent execution in seconds.
pub const SUBAGENT_TIMEOUT_SECS: u64 = 300;

/// Configuration for subagent spawning limits
#[derive(Debug, Clone)]
pub struct SubagentConfig {
    pub max_depth: u32,
    pub max_budget: ExecutionBudget,
    /// Optional per-subagent memory limit in MB. Defaults to `MAX_MEMORY_PER_SUBAGENT_MB`.
    pub memory_limit_mb: Option<usize>,
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            max_depth: 5,
            max_budget: ExecutionBudget {
                max_steps: Some(1000),
                max_duration_ms: Some(300000), // 5 minutes
                max_tokens: Some(100000),
                max_children: Some(10),
                tokens_used: 0,
            },
            memory_limit_mb: None,
        }
    }
}

impl SubagentConfig {
    /// Minimum depth (must allow at least root execution)
    const MIN_DEPTH: u32 = 1;
    /// Maximum depth (prevents infinite recursion)
    const MAX_DEPTH: u32 = 20;
    /// Maximum steps (prevents resource exhaustion)
    const MAX_STEPS: u32 = 100000;
    /// Maximum duration in milliseconds (10 minutes)
    const MAX_DURATION_MS: u64 = 600000;
    /// Maximum tokens (prevents token exhaustion)
    const MAX_TOKENS: u32 = 1000000;
    /// Maximum children (prevents fork bombs)
    const MAX_CHILDREN: u32 = 100;

    /// Validate configuration values
    ///
    /// # Security
    /// Validates limits to prevent:
    /// - Infinite recursion via excessive depth
    /// - Resource exhaustion via unbounded budgets
    /// - Fork bombs via excessive children
    pub fn validate(&self) -> anyhow::Result<()> {
        // Validate depth
        if self.max_depth < Self::MIN_DEPTH {
            anyhow::bail!(
                "max_depth too low: {} (min {})",
                self.max_depth,
                Self::MIN_DEPTH
            );
        }
        if self.max_depth > Self::MAX_DEPTH {
            anyhow::bail!(
                "max_depth too high: {} (max {})",
                self.max_depth,
                Self::MAX_DEPTH
            );
        }

        // Validate budget limits
        if let Some(steps) = self.max_budget.max_steps {
            if steps > Self::MAX_STEPS {
                anyhow::bail!(
                    "max_budget.max_steps too high: {} (max {})",
                    steps,
                    Self::MAX_STEPS
                );
            }
        }

        if let Some(duration) = self.max_budget.max_duration_ms {
            if duration > Self::MAX_DURATION_MS {
                anyhow::bail!(
                    "max_budget.max_duration_ms too high: {} (max {})",
                    duration,
                    Self::MAX_DURATION_MS
                );
            }
        }

        if let Some(tokens) = self.max_budget.max_tokens {
            if tokens > Self::MAX_TOKENS {
                anyhow::bail!(
                    "max_budget.max_tokens too high: {} (max {})",
                    tokens,
                    Self::MAX_TOKENS
                );
            }
        }

        if let Some(children) = self.max_budget.max_children {
            if children > Self::MAX_CHILDREN {
                anyhow::bail!(
                    "max_budget.max_children too high: {} (max {})",
                    children,
                    Self::MAX_CHILDREN
                );
            }
        }

        Ok(())
    }
}

pub trait SubagentScheduler: Send + Sync {
    fn spawn(&self, request: SpawnRequest) -> anyhow::Result<ExecutionId>;
    fn get_depth(&self, execution_id: &ExecutionId) -> anyhow::Result<u32>;
}

/// Snapshot of current resource consumption for the scheduler.
#[derive(Debug, Clone)]
pub struct ResourceStats {
    /// Number of currently active subagent executions.
    pub active_subagents: usize,
    /// Total memory allocated to active subagents in MB.
    pub total_memory_mb: usize,
    /// Remaining capacity in terms of subagent count slots.
    pub available_slots: usize,
    /// Remaining memory capacity in MB.
    pub available_memory_mb: usize,
}

/// InMemory implementation of SubagentScheduler with depth, budget, and memory validation.
#[derive(Debug, Clone)]
pub struct InMemorySubagentScheduler {
    executions: Arc<RwLock<BTreeMap<ExecutionId, Execution>>>,
    /// Tracks the memory (MB) allocated to each active execution.
    execution_memory: Arc<RwLock<BTreeMap<ExecutionId, usize>>>,
    /// Running total of allocated memory across all active executions.
    total_memory_mb: Arc<AtomicUsize>,
    config: SubagentConfig,
}

impl InMemorySubagentScheduler {
    pub fn new(config: SubagentConfig) -> Self {
        Self {
            executions: Arc::new(RwLock::new(BTreeMap::new())),
            execution_memory: Arc::new(RwLock::new(BTreeMap::new())),
            total_memory_mb: Arc::new(AtomicUsize::new(0)),
            config,
        }
    }

    fn validate_depth(
        &self,
        execution_id: &ExecutionId,
        executions: &BTreeMap<ExecutionId, Execution>,
    ) -> anyhow::Result<u32> {
        let mut current_id = Some(execution_id.clone());
        let mut depth = 0;

        // Count the number of ancestors (depth from root, where root has depth 0)
        while let Some(id) = current_id {
            let execution = executions
                .get(&id)
                .ok_or_else(|| anyhow::anyhow!("execution not found: {}", id))?;

            // Move to parent
            current_id = execution.parent_execution_id.clone();

            // Increment depth for each parent link we traverse
            if current_id.is_some() {
                depth += 1;
            }
        }

        Ok(depth)
    }

    fn validate_budget(&self, requested: &ExecutionBudget) -> anyhow::Result<()> {
        let max = &self.config.max_budget;

        if let (Some(req_steps), Some(max_steps)) = (requested.max_steps, max.max_steps) {
            if req_steps > max_steps {
                anyhow::bail!(
                    "requested steps {} exceeds maximum {}",
                    req_steps,
                    max_steps
                );
            }
        }

        if let (Some(req_duration), Some(max_duration)) =
            (requested.max_duration_ms, max.max_duration_ms)
        {
            if req_duration > max_duration {
                anyhow::bail!(
                    "requested duration {} exceeds maximum {}",
                    req_duration,
                    max_duration
                );
            }
        }

        if let (Some(req_tokens), Some(max_tokens)) = (requested.max_tokens, max.max_tokens) {
            if req_tokens > max_tokens {
                anyhow::bail!(
                    "requested tokens {} exceeds maximum {}",
                    req_tokens,
                    max_tokens
                );
            }
        }

        if let (Some(req_children), Some(max_children)) = (requested.max_children, max.max_children)
        {
            if req_children > max_children {
                anyhow::bail!(
                    "requested children {} exceeds maximum {}",
                    req_children,
                    max_children
                );
            }
        }

        Ok(())
    }
}

impl Default for InMemorySubagentScheduler {
    fn default() -> Self {
        Self::new(SubagentConfig::default())
    }
}

impl InMemorySubagentScheduler {
    /// Terminate a subagent execution and release its reserved memory.
    ///
    /// Returns `Ok(())` even if the execution ID is not found (idempotent).
    pub fn terminate_subagent(&self, id: &ExecutionId) -> anyhow::Result<()> {
        let mut executions = self
            .executions
            .try_write()
            .map_err(|_| anyhow::anyhow!("failed to acquire write lock on executions"))?;
        let mut memory_map = self
            .execution_memory
            .try_write()
            .map_err(|_| anyhow::anyhow!("failed to acquire write lock on memory map"))?;

        executions.remove(id);
        if let Some(mem) = memory_map.remove(id) {
            self.total_memory_mb.fetch_sub(mem, Ordering::SeqCst);
        }
        Ok(())
    }

    /// Return a snapshot of current resource usage.
    pub fn resource_stats(&self) -> anyhow::Result<ResourceStats> {
        let executions = self
            .executions
            .try_read()
            .map_err(|_| anyhow::anyhow!("failed to acquire read lock on executions"))?;
        let active = executions.len();
        let used_memory = self.total_memory_mb.load(Ordering::Acquire);
        Ok(ResourceStats {
            active_subagents: active,
            total_memory_mb: used_memory,
            available_slots: MAX_SUBAGENTS.saturating_sub(active),
            available_memory_mb: MAX_TOTAL_MEMORY_MB.saturating_sub(used_memory),
        })
    }
}

impl SubagentScheduler for InMemorySubagentScheduler {
    fn spawn(&self, request: SpawnRequest) -> anyhow::Result<ExecutionId> {
        let mut executions = self
            .executions
            .try_write()
            .map_err(|_| anyhow::anyhow!("failed to acquire write lock"))?;

        // Check subagent count limit
        if executions.len() >= MAX_SUBAGENTS {
            anyhow::bail!("maximum subagent count ({}) reached", MAX_SUBAGENTS);
        }

        // Validate depth: check if child would exceed max depth
        // child_depth = parent_depth + 1
        let parent_depth = self.validate_depth(&request.parent_execution_id, &executions)?;
        let child_depth = parent_depth + 1;
        if child_depth > self.config.max_depth {
            anyhow::bail!(
                "maximum execution depth exceeded: child would have depth {} > max {}",
                child_depth,
                self.config.max_depth
            );
        }

        // Validate budget
        self.validate_budget(&request.budget)?;

        // Determine requested memory for this subagent
        let requested_memory = self
            .config
            .memory_limit_mb
            .unwrap_or(MAX_MEMORY_PER_SUBAGENT_MB);

        // Validate per-subagent memory ceiling
        if requested_memory > MAX_MEMORY_PER_SUBAGENT_MB {
            anyhow::bail!(
                "requested memory ({} MB) exceeds per-subagent limit ({} MB)",
                requested_memory,
                MAX_MEMORY_PER_SUBAGENT_MB
            );
        }

        // Validate total memory ceiling
        let current_memory = self.total_memory_mb.load(Ordering::Acquire);
        if current_memory + requested_memory > MAX_TOTAL_MEMORY_MB {
            anyhow::bail!(
                "total memory limit ({} MB) would be exceeded (current: {} MB, requested: {} MB)",
                MAX_TOTAL_MEMORY_MB,
                current_memory,
                requested_memory
            );
        }

        // Get root execution ID from parent
        let parent = executions
            .get(&request.parent_execution_id)
            .ok_or_else(|| anyhow::anyhow!("parent execution not found"))?;
        let root_execution_id = parent.root_execution_id.clone();
        let trace_id = parent.trace_id.clone();

        // Create new execution
        let execution_id = ExecutionId::new();
        let execution = Execution {
            id: execution_id.clone(),
            root_execution_id,
            parent_execution_id: Some(request.parent_execution_id),
            owner_node_id: None,
            scheduled_node_id: None,
            placement_group: None,
            lease_id: None,
            handoff_count: 0,
            case_id: None,
            task_id: Some(request.task.id),
            agent: AgentRef {
                id: request.target_agent_id,
                role: "subagent".to_string(),
            },
            status: ExecutionStatus::Pending,
            join_strategy: None,
            budget: request.budget,
            workspace: request.context.workspace,
            trace_id,
            started_at: None,
            finished_at: None,
            risk_level: cyberclaw_core::capability::RiskLevel::Low, // Will be inherited from parent or updated during execution
            execution_mode: cyberclaw_core::execution::ExecutionMode::Normal,
        };

        executions.insert(execution_id.clone(), execution);

        // Record memory allocation for this execution
        {
            let mut memory_map = self
                .execution_memory
                .try_write()
                .map_err(|_| anyhow::anyhow!("failed to acquire write lock on memory map"))?;
            memory_map.insert(execution_id.clone(), requested_memory);
        }
        self.total_memory_mb
            .fetch_add(requested_memory, Ordering::SeqCst);

        Ok(execution_id)
    }

    fn get_depth(&self, execution_id: &ExecutionId) -> anyhow::Result<u32> {
        let executions = self
            .executions
            .try_read()
            .map_err(|_| anyhow::anyhow!("failed to acquire read lock"))?;
        self.validate_depth(execution_id, &executions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use cyberclaw_core::identity::ActorType;
    use cyberclaw_core::ids::ActorId;

    fn create_test_actor(id: &str, display_name: &str, actor_type: ActorType) -> ActorRef {
        ActorRef {
            id: ActorId::from_string(id.to_string()).unwrap(),
            actor_type,
            tenant_id: None,
            home_node_id: None,
            display_name: display_name.to_string(),
        }
    }

    #[test]
    fn test_spawn_subagent() {
        let scheduler = InMemorySubagentScheduler::default();

        // Create parent execution first
        let parent_id = ExecutionId::new();
        let parent = Execution {
            id: parent_id.clone(),
            root_execution_id: parent_id.clone(),
            parent_execution_id: None,
            owner_node_id: None,
            scheduled_node_id: None,
            placement_group: None,
            lease_id: None,
            handoff_count: 0,
            case_id: None,
            task_id: None,
            agent: AgentRef {
                id: AgentId::from_string("parent-agent".to_string()).unwrap(),
                role: "parent".to_string(),
            },
            status: ExecutionStatus::Running,
            join_strategy: None,
            budget: ExecutionBudget::default(),
            workspace: None,
            trace_id: TraceId::new(),
            risk_level: cyberclaw_core::capability::RiskLevel::Low,
            execution_mode: cyberclaw_core::execution::ExecutionMode::Normal,
            started_at: Some(Utc::now()),
            finished_at: None,
        };

        scheduler
            .executions
            .try_write()
            .unwrap()
            .insert(parent_id.clone(), parent);

        let spawn_request = SpawnRequest {
            parent_execution_id: parent_id,
            requesting_agent_id: AgentId::from_string("parent-agent".to_string()).unwrap(),
            target_agent_id: AgentId::from_string("child-agent".to_string()).unwrap(),
            task: Task {
                id: TaskId::new(),
                case_id: None,
                title: "Child Task".to_string(),
                summary: "Test".to_string(),
                kind: TaskKind::Analysis,
                priority: Priority::Medium,
                requested_by: create_test_actor("parent-agent", "Parent Agent", ActorType::Agent),
                requested_at: Utc::now(),
                trigger: TriggerRef {
                    kind: "spawn".to_string(),
                    source: "parent".to_string(),
                },
                input: TaskInput::default(),
                desired_outputs: vec![],
                labels: vec![],
                preferred_agent_id: None,
            },
            context: ContextPack::default(),
            budget: ExecutionBudget {
                max_steps: Some(100),
                max_duration_ms: Some(60000),
                max_tokens: Some(10000),
                max_children: Some(5),
                tokens_used: 0,
            },
            workspace_mode: WorkspaceMode::Ephemeral,
            priority: Priority::Medium,
        };

        let child_id = scheduler.spawn(spawn_request).unwrap();
        let depth = scheduler.get_depth(&child_id).unwrap();
        assert_eq!(depth, 1);
    }

    #[test]
    fn test_depth_limit_exceeded() {
        let scheduler = InMemorySubagentScheduler::new(SubagentConfig {
            max_depth: 1,
            ..Default::default()
        });

        // Create chain: root -> child1 -> child2
        let root_id = ExecutionId::new();
        let root = Execution {
            id: root_id.clone(),
            root_execution_id: root_id.clone(),
            parent_execution_id: None,
            owner_node_id: None,
            scheduled_node_id: None,
            placement_group: None,
            lease_id: None,
            handoff_count: 0,
            case_id: None,
            task_id: None,
            agent: AgentRef {
                id: AgentId::from_string("root".to_string()).unwrap(),
                role: "root".to_string(),
            },
            status: ExecutionStatus::Running,
            join_strategy: None,
            budget: ExecutionBudget::default(),
            workspace: None,
            trace_id: TraceId::new(),
            risk_level: cyberclaw_core::capability::RiskLevel::Low,
            execution_mode: cyberclaw_core::execution::ExecutionMode::Normal,
            started_at: Some(Utc::now()),
            finished_at: None,
        };

        let child1_id = ExecutionId::new();
        let child1 = Execution {
            id: child1_id.clone(),
            root_execution_id: root_id.clone(),
            parent_execution_id: Some(root_id.clone()),
            owner_node_id: None,
            scheduled_node_id: None,
            placement_group: None,
            lease_id: None,
            handoff_count: 0,
            case_id: None,
            task_id: None,
            agent: AgentRef {
                id: AgentId::from_string("child1".to_string()).unwrap(),
                role: "child".to_string(),
            },
            status: ExecutionStatus::Running,
            join_strategy: None,
            budget: ExecutionBudget::default(),
            workspace: None,
            trace_id: TraceId::new(),
            risk_level: cyberclaw_core::capability::RiskLevel::Low,
            execution_mode: cyberclaw_core::execution::ExecutionMode::Normal,
            started_at: Some(Utc::now()),
            finished_at: None,
        };

        {
            let mut executions = scheduler.executions.try_write().unwrap();
            executions.insert(root_id.clone(), root);
            executions.insert(child1_id.clone(), child1);
        }

        // Try to spawn child2 from child1 (would be depth 3, exceeds limit of 2)
        let spawn_request = SpawnRequest {
            parent_execution_id: child1_id,
            requesting_agent_id: AgentId::from_string("child1".to_string()).unwrap(),
            target_agent_id: AgentId::from_string("child2".to_string()).unwrap(),
            task: Task {
                id: TaskId::new(),
                case_id: None,
                title: "Child2 Task".to_string(),
                summary: "Test".to_string(),
                kind: TaskKind::Analysis,
                priority: Priority::Medium,
                requested_by: create_test_actor("child1", "Child1 Agent", ActorType::Agent),
                requested_at: Utc::now(),
                trigger: TriggerRef {
                    kind: "spawn".to_string(),
                    source: "child1".to_string(),
                },
                input: TaskInput::default(),
                desired_outputs: vec![],
                labels: vec![],
                preferred_agent_id: None,
            },
            context: ContextPack::default(),
            budget: ExecutionBudget::default(),
            workspace_mode: WorkspaceMode::Ephemeral,
            priority: Priority::Medium,
        };

        let result = scheduler.spawn(spawn_request);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("maximum execution depth exceeded"));
    }

    #[test]
    fn test_budget_validation() {
        let scheduler = InMemorySubagentScheduler::default();

        let excessive_budget = ExecutionBudget {
            max_steps: Some(10000), // Exceeds default limit of 1000
            max_duration_ms: None,
            max_tokens: None,
            max_children: None,
            tokens_used: 0,
        };

        let result = scheduler.validate_budget(&excessive_budget);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("exceeds maximum"));
    }

    // Configuration Validation Tests

    #[test]
    fn test_config_default_is_valid() {
        let config = SubagentConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_depth_too_low() {
        let config = SubagentConfig {
            max_depth: 0, // Below MIN (1)
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("max_depth too low"));
    }

    #[test]
    fn test_config_depth_too_high() {
        let config = SubagentConfig {
            max_depth: 21, // Above MAX (20)
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("max_depth too high"));
    }

    #[test]
    fn test_config_steps_too_high() {
        let config = SubagentConfig {
            max_depth: 5,
            max_budget: ExecutionBudget {
                max_steps: Some(100001), // Above MAX (100000)
                ..Default::default()
            },
            memory_limit_mb: None,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("max_budget.max_steps too high"));
    }

    #[test]
    fn test_config_duration_too_high() {
        let config = SubagentConfig {
            max_depth: 5,
            max_budget: ExecutionBudget {
                max_duration_ms: Some(600001), // Above MAX (600000)
                ..Default::default()
            },
            memory_limit_mb: None,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("max_budget.max_duration_ms too high"));
    }

    #[test]
    fn test_config_tokens_too_high() {
        let config = SubagentConfig {
            max_depth: 5,
            max_budget: ExecutionBudget {
                max_tokens: Some(1000001), // Above MAX (1000000)
                ..Default::default()
            },
            memory_limit_mb: None,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("max_budget.max_tokens too high"));
    }

    #[test]
    fn test_config_children_too_high() {
        let config = SubagentConfig {
            max_depth: 5,
            max_budget: ExecutionBudget {
                max_children: Some(101), // Above MAX (100)
                ..Default::default()
            },
            memory_limit_mb: None,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("max_budget.max_children too high"));
    }

    #[test]
    fn test_config_valid_edge_cases() {
        // Min depth
        let config = SubagentConfig {
            max_depth: 1,
            ..Default::default()
        };
        assert!(config.validate().is_ok());

        // Max depth
        let config = SubagentConfig {
            max_depth: 20,
            ..Default::default()
        };
        assert!(config.validate().is_ok());

        // Max budget values
        let config = SubagentConfig {
            max_depth: 5,
            max_budget: ExecutionBudget {
                max_steps: Some(100000),
                max_duration_ms: Some(600000),
                max_tokens: Some(1000000),
                max_children: Some(100),
                tokens_used: 0,
            },
            memory_limit_mb: None,
        };
        assert!(config.validate().is_ok());
    }
}
