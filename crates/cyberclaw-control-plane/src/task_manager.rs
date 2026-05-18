use cyberclaw_core::prelude::*;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[async_trait::async_trait]
pub trait TaskManager: Send + Sync {
    async fn create_task(&self, task: Task) -> anyhow::Result<Task>;
    async fn get_task(&self, id: &TaskId) -> anyhow::Result<Option<Task>>;
    async fn list_tasks(&self) -> anyhow::Result<Vec<Task>>;
}

/// InMemory implementation of TaskManager for development and testing
#[derive(Debug, Clone)]
pub struct InMemoryTaskManager {
    tasks: Arc<Mutex<BTreeMap<TaskId, Task>>>,
    max_capacity: usize,
}

impl Default for InMemoryTaskManager {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryTaskManager {
    /// Create a new TaskManager with default capacity of 10000
    pub fn new() -> Self {
        Self::with_capacity(10000)
    }

    /// Create a new TaskManager with specified capacity
    pub fn with_capacity(max_capacity: usize) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(BTreeMap::new())),
            max_capacity,
        }
    }
}

#[async_trait::async_trait]
impl TaskManager for InMemoryTaskManager {
    async fn create_task(&self, task: Task) -> anyhow::Result<Task> {
        // Validate task fields to prevent injection attacks
        task.validate()?;

        let mut tasks = self.tasks.lock().await;

        // Enforce capacity limit to prevent DoS attacks
        if tasks.len() >= self.max_capacity {
            anyhow::bail!(
                "task manager at capacity: {} (limit: {})",
                tasks.len(),
                self.max_capacity
            );
        }

        if tasks.contains_key(&task.id) {
            anyhow::bail!("task already exists: {}", task.id);
        }

        tasks.insert(task.id.clone(), task.clone());
        Ok(task)
    }

    async fn get_task(&self, id: &TaskId) -> anyhow::Result<Option<Task>> {
        let tasks = self.tasks.lock().await;
        Ok(tasks.get(id).cloned())
    }

    async fn list_tasks(&self) -> anyhow::Result<Vec<Task>> {
        let tasks = self.tasks.lock().await;
        Ok(tasks.values().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use cyberclaw_core::identity::ActorType;
    use cyberclaw_core::ids::ActorId;

    fn create_test_actor(id: &str, display_name: &str) -> ActorRef {
        ActorRef {
            id: ActorId::from_string(id.to_string()).unwrap(),
            actor_type: ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: display_name.to_string(),
        }
    }

    #[tokio::test]
    async fn test_create_and_get_task() {
        let manager = InMemoryTaskManager::new();
        let task = Task {
            id: TaskId::new(),
            case_id: None,
            title: "Test Task".to_string(),
            summary: "Test summary".to_string(),
            kind: TaskKind::Analysis,
            priority: Priority::Medium,
            requested_by: create_test_actor("test-user", "Test User"),
            requested_at: Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        let task_id = task.id.clone();
        manager.create_task(task.clone()).await.unwrap();

        let retrieved = manager.get_task(&task_id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().title, "Test Task");
    }

    #[tokio::test]
    async fn test_create_duplicate_task_fails() {
        let manager = InMemoryTaskManager::new();
        let task = Task {
            id: TaskId::new(),
            case_id: None,
            title: "Test Task".to_string(),
            summary: "Test summary".to_string(),
            kind: TaskKind::Analysis,
            priority: Priority::Medium,
            requested_by: create_test_actor("test-user", "Test User"),
            requested_at: Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        manager.create_task(task.clone()).await.unwrap();
        let result = manager.create_task(task).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_tasks() {
        let manager = InMemoryTaskManager::new();

        for i in 0..3 {
            let task = Task {
                id: TaskId::new(),
                case_id: None,
                title: format!("Task {}", i),
                summary: "Test summary".to_string(),
                kind: TaskKind::Analysis,
                priority: Priority::Medium,
                requested_by: create_test_actor("test-user", "Test User"),
                requested_at: Utc::now(),
                trigger: TriggerRef {
                    kind: "manual".to_string(),
                    source: "test".to_string(),
                },
                input: TaskInput::default(),
                desired_outputs: vec![],
                labels: vec![],
                preferred_agent_id: None,
            };
            manager.create_task(task).await.unwrap();
        }

        let tasks = manager.list_tasks().await.unwrap();
        assert_eq!(tasks.len(), 3);
    }

    #[tokio::test]
    async fn test_capacity_limit_enforced() {
        // Create a manager with small capacity for testing
        let manager = InMemoryTaskManager::with_capacity(3);

        // Fill the manager to capacity
        for i in 0..3 {
            let task = Task {
                id: TaskId::new(),
                case_id: None,
                title: format!("Task {}", i),
                summary: "Test summary".to_string(),
                kind: TaskKind::Analysis,
                priority: Priority::Medium,
                requested_by: create_test_actor("test-user", "Test User"),
                requested_at: Utc::now(),
                trigger: TriggerRef {
                    kind: "manual".to_string(),
                    source: "test".to_string(),
                },
                input: TaskInput::default(),
                desired_outputs: vec![],
                labels: vec![],
                preferred_agent_id: None,
            };
            manager.create_task(task).await.unwrap();
        }

        // Attempt to add one more should fail
        let overflow_task = Task {
            id: TaskId::new(),
            case_id: None,
            title: "Overflow Task".to_string(),
            summary: "Should be rejected".to_string(),
            kind: TaskKind::Analysis,
            priority: Priority::Medium,
            requested_by: create_test_actor("test-user", "Test User"),
            requested_at: Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        let result = manager.create_task(overflow_task).await;
        assert!(result.is_err(), "should reject when at capacity");
        assert!(
            result.unwrap_err().to_string().contains("at capacity"),
            "error message should mention capacity"
        );
    }

    #[tokio::test]
    async fn test_capacity_limit_allows_within_limit() {
        let manager = InMemoryTaskManager::with_capacity(5);

        // Add tasks within capacity
        for i in 0..5 {
            let task = Task {
                id: TaskId::new(),
                case_id: None,
                title: format!("Task {}", i),
                summary: "Test summary".to_string(),
                kind: TaskKind::Analysis,
                priority: Priority::Medium,
                requested_by: create_test_actor("test-user", "Test User"),
                requested_at: Utc::now(),
                trigger: TriggerRef {
                    kind: "manual".to_string(),
                    source: "test".to_string(),
                },
                input: TaskInput::default(),
                desired_outputs: vec![],
                labels: vec![],
                preferred_agent_id: None,
            };
            manager
                .create_task(task)
                .await
                .expect("should succeed within capacity");
        }

        let tasks = manager.list_tasks().await.unwrap();
        assert_eq!(tasks.len(), 5, "all tasks should be created");
    }

    #[tokio::test]
    async fn test_validation_rejects_empty_title() {
        let manager = InMemoryTaskManager::new();
        let task = Task {
            id: TaskId::new(),
            case_id: None,
            title: "".to_string(), // Empty title
            summary: "Test summary".to_string(),
            kind: TaskKind::Analysis,
            priority: Priority::Medium,
            requested_by: create_test_actor("test-user", "Test User"),
            requested_at: Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        let result = manager.create_task(task).await;
        assert!(result.is_err(), "should reject empty title");
        assert!(
            result.unwrap_err().to_string().contains("at least 1"),
            "error should mention minimum length"
        );
    }

    #[tokio::test]
    async fn test_validation_rejects_title_too_long() {
        let manager = InMemoryTaskManager::new();
        let long_title = "A".repeat(256); // Exceeds 255 character limit
        let task = Task {
            id: TaskId::new(),
            case_id: None,
            title: long_title,
            summary: "Test summary".to_string(),
            kind: TaskKind::Analysis,
            priority: Priority::Medium,
            requested_by: create_test_actor("test-user", "Test User"),
            requested_at: Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        let result = manager.create_task(task).await;
        assert!(result.is_err(), "should reject title exceeding max length");
        assert!(
            result.unwrap_err().to_string().contains("maximum length"),
            "error should mention maximum length"
        );
    }

    #[tokio::test]
    async fn test_validation_rejects_control_characters() {
        let manager = InMemoryTaskManager::new();
        let task = Task {
            id: TaskId::new(),
            case_id: None,
            title: "Task with \u{0000} null byte".to_string(), // Contains null byte
            summary: "Test summary".to_string(),
            kind: TaskKind::Analysis,
            priority: Priority::Medium,
            requested_by: create_test_actor("test-user", "Test User"),
            requested_at: Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        let result = manager.create_task(task).await;
        assert!(result.is_err(), "should reject control characters");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("control characters"),
            "error should mention control characters"
        );
    }

    #[tokio::test]
    async fn test_validation_rejects_summary_too_long() {
        let manager = InMemoryTaskManager::new();
        let long_summary = "A".repeat(2001); // Exceeds 2000 character limit
        let task = Task {
            id: TaskId::new(),
            case_id: None,
            title: "Test Task".to_string(),
            summary: long_summary,
            kind: TaskKind::Analysis,
            priority: Priority::Medium,
            requested_by: create_test_actor("test-user", "Test User"),
            requested_at: Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        let result = manager.create_task(task).await;
        assert!(
            result.is_err(),
            "should reject summary exceeding max length"
        );
        assert!(
            result.unwrap_err().to_string().contains("maximum length"),
            "error should mention maximum length"
        );
    }

    #[tokio::test]
    async fn test_validation_allows_valid_task() {
        let manager = InMemoryTaskManager::new();
        let task = Task {
            id: TaskId::new(),
            case_id: None,
            title: "Valid Task Title".to_string(),
            summary: "This is a valid summary with\nnewlines\tand\ttabs".to_string(),
            kind: TaskKind::Analysis,
            priority: Priority::Medium,
            requested_by: create_test_actor("test-user", "Test User"),
            requested_at: Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec!["tag1".to_string(), "tag2".to_string()],
            preferred_agent_id: None,
        };

        let result = manager.create_task(task).await;
        assert!(
            result.is_ok(),
            "should accept valid task with newlines and tabs"
        );
    }
}
