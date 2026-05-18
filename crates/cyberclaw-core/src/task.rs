use crate::enums::Priority;
use crate::identity::ActorRef;
use crate::ids::{AgentId, CaseId, TaskId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskKind {
    Analysis,
    Investigation,
    Review,
    Execution,
    Reporting,
    Automation,
    Autopilot, // Agent 6: Autopilot task type for autonomous multi-iteration execution
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerRef {
    pub kind: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskInput {
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputContractRef {
    pub schema: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub case_id: Option<CaseId>,
    pub title: String,
    pub summary: String,
    pub kind: TaskKind,
    pub priority: Priority,
    pub requested_by: ActorRef,
    pub requested_at: chrono::DateTime<chrono::Utc>,
    pub trigger: TriggerRef,
    pub input: TaskInput,
    pub desired_outputs: Vec<OutputContractRef>,
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_agent_id: Option<AgentId>,
}

impl Task {
    /// Validate task fields to prevent injection attacks and ensure data integrity
    pub fn validate(&self) -> anyhow::Result<()> {
        // Validate title
        Self::validate_string_field(&self.title, "title", 1, 255)?;

        // Validate summary
        Self::validate_string_field(&self.summary, "summary", 0, 2000)?;

        // Validate labels
        for (i, label) in self.labels.iter().enumerate() {
            Self::validate_string_field(label, &format!("labels[{}]", i), 1, 100)?;
        }

        Ok(())
    }

    /// Validate a string field for length and control characters
    fn validate_string_field(
        value: &str,
        field_name: &str,
        min_len: usize,
        max_len: usize,
    ) -> anyhow::Result<()> {
        // Check minimum length
        if value.len() < min_len {
            anyhow::bail!(
                "{} must be at least {} characters (got: {})",
                field_name,
                min_len,
                value.len()
            );
        }

        // Check maximum length to prevent DoS
        if value.len() > max_len {
            anyhow::bail!(
                "{} exceeds maximum length of {} (got: {})",
                field_name,
                max_len,
                value.len()
            );
        }

        // Check for control characters (except newline, tab, carriage return)
        // This prevents injection attacks and ensures printable text
        let has_control_chars = value
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t' && c != '\r');

        if has_control_chars {
            anyhow::bail!("{} contains invalid control characters", field_name);
        }

        Ok(())
    }
}
