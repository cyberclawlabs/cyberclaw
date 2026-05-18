//! Core request/response types for the agent runtime.

use cyberclaw_core::ids::AgentId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A request to execute an agent action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    /// The target agent to execute.
    pub agent_id: AgentId,
    /// The action or prompt to execute.
    pub input: String,
    /// Optional key-value context passed to the agent.
    #[serde(default)]
    pub context: HashMap<String, serde_json::Value>,
}

impl AgentRequest {
    /// Create a new request with just an agent ID and input.
    pub fn new(agent_id: AgentId, input: impl Into<String>) -> Self {
        Self {
            agent_id,
            input: input.into(),
            context: HashMap::new(),
        }
    }

    /// Attach additional context to the request.
    pub fn with_context(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.context.insert(key.into(), value);
        self
    }
}

/// The response produced by an agent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// The agent that produced this response.
    pub agent_id: AgentId,
    /// The output produced by the agent.
    pub output: String,
    /// Indicates whether the execution succeeded.
    pub success: bool,
    /// Optional metadata returned by the agent.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl AgentResponse {
    /// Construct a successful response.
    pub fn ok(agent_id: AgentId, output: impl Into<String>) -> Self {
        Self {
            agent_id,
            output: output.into(),
            success: true,
            metadata: HashMap::new(),
        }
    }

    /// Construct a failed response with an error message.
    pub fn err(agent_id: AgentId, message: impl Into<String>) -> Self {
        Self {
            agent_id,
            output: message.into(),
            success: false,
            metadata: HashMap::new(),
        }
    }
}
