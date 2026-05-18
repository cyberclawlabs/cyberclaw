//! Workflow trigger system
//!
//! Provides trigger types, registration, and event matching for workflow automation.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

/// Trigger types that can initiate a workflow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkflowTrigger {
    Manual,
    OnExecutionComplete {
        execution_id_pattern: String,
    },
    OnWebhook {
        platform: String,
        event_type: Option<String>,
    },
    Cron {
        expression: String,
        timezone: Option<String>,
    },
    OnEvent {
        event_type: String,
        filter: Option<serde_json::Value>,
    },
}

/// A registered trigger bound to a workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerRegistration {
    pub trigger_id: String,
    pub workflow_id: String,
    pub trigger: WorkflowTrigger,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

/// An incoming event to match against registered triggers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerEvent {
    ExecutionCompleted {
        execution_id: String,
    },
    WebhookReceived {
        platform: String,
        event_type: String,
        payload: serde_json::Value,
    },
    CronFired {
        expression: String,
    },
    CustomEvent {
        event_type: String,
        data: serde_json::Value,
    },
}

/// Pattern matching logic for triggers.
struct TriggerMatcher;

impl TriggerMatcher {
    /// Match a glob-like pattern against a string.
    /// Supports `*` as wildcard for any sequence of characters.
    fn glob_match(pattern: &str, value: &str) -> bool {
        let parts: Vec<&str> = pattern.split('*').collect();

        if parts.len() == 1 {
            // No wildcard, exact match
            return pattern == value;
        }

        let mut pos = 0;

        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() {
                continue;
            }
            match value[pos..].find(part) {
                Some(found) => {
                    // First segment must match at the start
                    if i == 0 && found != 0 {
                        return false;
                    }
                    pos += found + part.len();
                }
                None => return false,
            }
        }

        // If pattern doesn't end with *, the value must end at pos
        if !pattern.ends_with('*') {
            return pos == value.len();
        }

        true
    }

    /// Check if a trigger matches an event.
    fn matches(trigger: &WorkflowTrigger, event: &TriggerEvent) -> bool {
        match (trigger, event) {
            (
                WorkflowTrigger::OnExecutionComplete {
                    execution_id_pattern,
                },
                TriggerEvent::ExecutionCompleted { execution_id },
            ) => Self::glob_match(execution_id_pattern, execution_id),

            (
                WorkflowTrigger::OnWebhook {
                    platform: trigger_platform,
                    event_type: trigger_event_type,
                },
                TriggerEvent::WebhookReceived {
                    platform,
                    event_type,
                    ..
                },
            ) => {
                if trigger_platform != platform {
                    return false;
                }
                match trigger_event_type {
                    Some(t) => t == event_type,
                    None => true, // No event_type filter means match all events for this platform
                }
            }

            (
                WorkflowTrigger::Cron { expression, .. },
                TriggerEvent::CronFired {
                    expression: fired_expr,
                },
            ) => expression == fired_expr,

            (
                WorkflowTrigger::OnEvent {
                    event_type: trigger_type,
                    filter,
                },
                TriggerEvent::CustomEvent {
                    event_type: ev_type,
                    data,
                },
            ) => {
                if trigger_type != ev_type {
                    return false;
                }
                // If a filter is specified, every key-value pair in the filter
                // must exist in the event data with an equal value.
                if let Some(filter_obj) = filter {
                    if let Some(filter_map) = filter_obj.as_object() {
                        for (key, expected) in filter_map {
                            match data.get(key) {
                                Some(actual) => {
                                    if actual != expected {
                                        return false;
                                    }
                                }
                                None => return false,
                            }
                        }
                    }
                }
                true
            }

            _ => false,
        }
    }
}

/// Registry for managing workflow triggers.
#[derive(Clone)]
pub struct TriggerRegistry {
    triggers: Arc<RwLock<HashMap<String, TriggerRegistration>>>,
}

impl TriggerRegistry {
    /// Create a new empty trigger registry.
    pub fn new() -> Self {
        Self {
            triggers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a trigger for a workflow. Returns the trigger_id.
    pub async fn register(
        &self,
        workflow_id: &str,
        trigger: WorkflowTrigger,
    ) -> Result<String, TriggerError> {
        let trigger_id = Uuid::new_v4().to_string();
        let registration = TriggerRegistration {
            trigger_id: trigger_id.clone(),
            workflow_id: workflow_id.to_string(),
            trigger,
            enabled: true,
            created_at: Utc::now(),
        };
        let mut triggers = self.triggers.write().await;
        triggers.insert(trigger_id.clone(), registration);
        Ok(trigger_id)
    }

    /// Unregister a trigger by its ID.
    pub async fn unregister(&self, trigger_id: &str) -> Result<(), TriggerError> {
        let mut triggers = self.triggers.write().await;
        triggers
            .remove(trigger_id)
            .ok_or_else(|| TriggerError::TriggerNotFound(trigger_id.to_string()))?;
        Ok(())
    }

    /// Get all triggers for a given workflow.
    pub async fn get_triggers(&self, workflow_id: &str) -> Vec<TriggerRegistration> {
        let triggers = self.triggers.read().await;
        triggers
            .values()
            .filter(|t| t.workflow_id == workflow_id)
            .cloned()
            .collect()
    }

    /// Find all enabled triggers that match a given event.
    pub async fn find_matching_triggers(&self, event: &TriggerEvent) -> Vec<TriggerRegistration> {
        let triggers = self.triggers.read().await;
        triggers
            .values()
            .filter(|t| t.enabled && TriggerMatcher::matches(&t.trigger, event))
            .cloned()
            .collect()
    }

    /// Enable or disable a trigger by its ID.
    pub async fn set_enabled(&self, trigger_id: &str, enabled: bool) -> Result<(), TriggerError> {
        let mut triggers = self.triggers.write().await;
        let registration = triggers
            .get_mut(trigger_id)
            .ok_or_else(|| TriggerError::TriggerNotFound(trigger_id.to_string()))?;
        registration.enabled = enabled;
        Ok(())
    }
}

impl Default for TriggerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Trigger subsystem errors.
#[derive(Debug, thiserror::Error)]
pub enum TriggerError {
    #[error("trigger not found: {0}")]
    TriggerNotFound(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_and_get_trigger() {
        let registry = TriggerRegistry::new();
        let tid = registry
            .register("wf-1", WorkflowTrigger::Manual)
            .await
            .unwrap();
        let triggers = registry.get_triggers("wf-1").await;
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0].trigger_id, tid);
        assert_eq!(triggers[0].workflow_id, "wf-1");
    }

    #[tokio::test]
    async fn unregister_trigger() {
        let registry = TriggerRegistry::new();
        let tid = registry
            .register("wf-1", WorkflowTrigger::Manual)
            .await
            .unwrap();
        registry.unregister(&tid).await.unwrap();
        assert!(registry.get_triggers("wf-1").await.is_empty());
    }

    #[tokio::test]
    async fn unregister_nonexistent_returns_error() {
        let registry = TriggerRegistry::new();
        let result = registry.unregister("no-such-id").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn match_execution_complete_exact() {
        let registry = TriggerRegistry::new();
        registry
            .register(
                "wf-1",
                WorkflowTrigger::OnExecutionComplete {
                    execution_id_pattern: "exec-123".to_string(),
                },
            )
            .await
            .unwrap();

        let event = TriggerEvent::ExecutionCompleted {
            execution_id: "exec-123".to_string(),
        };
        let matched = registry.find_matching_triggers(&event).await;
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].workflow_id, "wf-1");
    }

    #[tokio::test]
    async fn match_execution_complete_glob_pattern() {
        let registry = TriggerRegistry::new();
        registry
            .register(
                "wf-1",
                WorkflowTrigger::OnExecutionComplete {
                    execution_id_pattern: "exec-*".to_string(),
                },
            )
            .await
            .unwrap();

        let event = TriggerEvent::ExecutionCompleted {
            execution_id: "exec-456".to_string(),
        };
        let matched = registry.find_matching_triggers(&event).await;
        assert_eq!(matched.len(), 1);

        // Non-matching
        let event2 = TriggerEvent::ExecutionCompleted {
            execution_id: "other-789".to_string(),
        };
        let matched2 = registry.find_matching_triggers(&event2).await;
        assert!(matched2.is_empty());
    }

    #[tokio::test]
    async fn match_webhook_trigger_platform_and_event() {
        let registry = TriggerRegistry::new();
        registry
            .register(
                "wf-1",
                WorkflowTrigger::OnWebhook {
                    platform: "github".to_string(),
                    event_type: Some("push".to_string()),
                },
            )
            .await
            .unwrap();

        let event = TriggerEvent::WebhookReceived {
            platform: "github".to_string(),
            event_type: "push".to_string(),
            payload: serde_json::json!({}),
        };
        let matched = registry.find_matching_triggers(&event).await;
        assert_eq!(matched.len(), 1);

        // Wrong event_type
        let event2 = TriggerEvent::WebhookReceived {
            platform: "github".to_string(),
            event_type: "pull_request".to_string(),
            payload: serde_json::json!({}),
        };
        assert!(registry.find_matching_triggers(&event2).await.is_empty());

        // Wrong platform
        let event3 = TriggerEvent::WebhookReceived {
            platform: "gitlab".to_string(),
            event_type: "push".to_string(),
            payload: serde_json::json!({}),
        };
        assert!(registry.find_matching_triggers(&event3).await.is_empty());
    }

    #[tokio::test]
    async fn match_webhook_trigger_platform_only() {
        let registry = TriggerRegistry::new();
        registry
            .register(
                "wf-1",
                WorkflowTrigger::OnWebhook {
                    platform: "github".to_string(),
                    event_type: None,
                },
            )
            .await
            .unwrap();

        // Should match any event_type on github
        let event = TriggerEvent::WebhookReceived {
            platform: "github".to_string(),
            event_type: "anything".to_string(),
            payload: serde_json::json!({}),
        };
        let matched = registry.find_matching_triggers(&event).await;
        assert_eq!(matched.len(), 1);
    }

    #[tokio::test]
    async fn match_cron_trigger() {
        let registry = TriggerRegistry::new();
        registry
            .register(
                "wf-1",
                WorkflowTrigger::Cron {
                    expression: "0 0 * * *".to_string(),
                    timezone: None,
                },
            )
            .await
            .unwrap();

        let event = TriggerEvent::CronFired {
            expression: "0 0 * * *".to_string(),
        };
        let matched = registry.find_matching_triggers(&event).await;
        assert_eq!(matched.len(), 1);

        let event2 = TriggerEvent::CronFired {
            expression: "0 12 * * *".to_string(),
        };
        assert!(registry.find_matching_triggers(&event2).await.is_empty());
    }

    #[tokio::test]
    async fn no_match_returns_empty() {
        let registry = TriggerRegistry::new();
        registry
            .register("wf-1", WorkflowTrigger::Manual)
            .await
            .unwrap();

        let event = TriggerEvent::ExecutionCompleted {
            execution_id: "exec-1".to_string(),
        };
        let matched = registry.find_matching_triggers(&event).await;
        assert!(matched.is_empty());
    }

    #[tokio::test]
    async fn multiple_triggers_same_workflow() {
        let registry = TriggerRegistry::new();
        registry
            .register("wf-1", WorkflowTrigger::Manual)
            .await
            .unwrap();
        registry
            .register(
                "wf-1",
                WorkflowTrigger::OnExecutionComplete {
                    execution_id_pattern: "exec-*".to_string(),
                },
            )
            .await
            .unwrap();
        registry
            .register(
                "wf-1",
                WorkflowTrigger::Cron {
                    expression: "0 0 * * *".to_string(),
                    timezone: None,
                },
            )
            .await
            .unwrap();

        let triggers = registry.get_triggers("wf-1").await;
        assert_eq!(triggers.len(), 3);
    }

    #[tokio::test]
    async fn enable_disable_trigger() {
        let registry = TriggerRegistry::new();
        let tid = registry
            .register(
                "wf-1",
                WorkflowTrigger::OnExecutionComplete {
                    execution_id_pattern: "exec-*".to_string(),
                },
            )
            .await
            .unwrap();

        let event = TriggerEvent::ExecutionCompleted {
            execution_id: "exec-1".to_string(),
        };

        // Enabled by default
        assert_eq!(registry.find_matching_triggers(&event).await.len(), 1);

        // Disable
        registry.set_enabled(&tid, false).await.unwrap();
        assert!(registry.find_matching_triggers(&event).await.is_empty());

        // Re-enable
        registry.set_enabled(&tid, true).await.unwrap();
        assert_eq!(registry.find_matching_triggers(&event).await.len(), 1);
    }

    #[tokio::test]
    async fn match_custom_event() {
        let registry = TriggerRegistry::new();
        registry
            .register(
                "wf-1",
                WorkflowTrigger::OnEvent {
                    event_type: "deployment.completed".to_string(),
                    filter: None,
                },
            )
            .await
            .unwrap();

        let event = TriggerEvent::CustomEvent {
            event_type: "deployment.completed".to_string(),
            data: serde_json::json!({"env": "prod"}),
        };
        let matched = registry.find_matching_triggers(&event).await;
        assert_eq!(matched.len(), 1);

        let event2 = TriggerEvent::CustomEvent {
            event_type: "deployment.failed".to_string(),
            data: serde_json::json!({}),
        };
        assert!(registry.find_matching_triggers(&event2).await.is_empty());
    }

    #[test]
    fn glob_match_middle_wildcard() {
        assert!(TriggerMatcher::glob_match("exec-*-final", "exec-123-final"));
        assert!(!TriggerMatcher::glob_match(
            "exec-*-final",
            "exec-123-other"
        ));
    }

    #[test]
    fn glob_match_no_wildcard() {
        assert!(TriggerMatcher::glob_match("exact", "exact"));
        assert!(!TriggerMatcher::glob_match("exact", "other"));
    }

    #[tokio::test]
    async fn match_custom_event_with_filter_passes() {
        let registry = TriggerRegistry::new();
        registry
            .register(
                "wf-filter",
                WorkflowTrigger::OnEvent {
                    event_type: "deploy.done".to_string(),
                    filter: Some(serde_json::json!({"env": "prod", "status": "ok"})),
                },
            )
            .await
            .unwrap();

        // All filter keys present with matching values → should match
        let event = TriggerEvent::CustomEvent {
            event_type: "deploy.done".to_string(),
            data: serde_json::json!({"env": "prod", "status": "ok", "region": "us-east-1"}),
        };
        let matched = registry.find_matching_triggers(&event).await;
        assert_eq!(matched.len(), 1);
    }

    #[tokio::test]
    async fn match_custom_event_with_filter_wrong_value() {
        let registry = TriggerRegistry::new();
        registry
            .register(
                "wf-filter",
                WorkflowTrigger::OnEvent {
                    event_type: "deploy.done".to_string(),
                    filter: Some(serde_json::json!({"env": "prod"})),
                },
            )
            .await
            .unwrap();

        // Filter value doesn't match → should not match
        let event = TriggerEvent::CustomEvent {
            event_type: "deploy.done".to_string(),
            data: serde_json::json!({"env": "staging"}),
        };
        assert!(registry.find_matching_triggers(&event).await.is_empty());
    }

    #[tokio::test]
    async fn match_custom_event_with_filter_missing_key() {
        let registry = TriggerRegistry::new();
        registry
            .register(
                "wf-filter",
                WorkflowTrigger::OnEvent {
                    event_type: "deploy.done".to_string(),
                    filter: Some(serde_json::json!({"env": "prod"})),
                },
            )
            .await
            .unwrap();

        // Key required by filter is absent in data → should not match
        let event = TriggerEvent::CustomEvent {
            event_type: "deploy.done".to_string(),
            data: serde_json::json!({"status": "ok"}),
        };
        assert!(registry.find_matching_triggers(&event).await.is_empty());
    }

    #[tokio::test]
    async fn match_custom_event_no_filter_still_works() {
        let registry = TriggerRegistry::new();
        registry
            .register(
                "wf-nofilter",
                WorkflowTrigger::OnEvent {
                    event_type: "deploy.done".to_string(),
                    filter: None,
                },
            )
            .await
            .unwrap();

        // No filter → matches on event_type alone
        let event = TriggerEvent::CustomEvent {
            event_type: "deploy.done".to_string(),
            data: serde_json::json!({"any": "data"}),
        };
        let matched = registry.find_matching_triggers(&event).await;
        assert_eq!(matched.len(), 1);
    }
}
