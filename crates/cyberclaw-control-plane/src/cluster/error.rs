use std::fmt;

#[derive(Debug)]
pub enum ClusterError {
    NoAvailableNodes,
    ConnectionError(String),
    ConsensusError(String),
    SerializationError(String),
    TimeError(String),
    NodeNotFound(uuid::Uuid),
}

impl fmt::Display for ClusterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClusterError::NoAvailableNodes => write!(f, "No available nodes"),
            ClusterError::ConnectionError(msg) => write!(f, "Connection error: {}", msg),
            ClusterError::ConsensusError(msg) => write!(f, "Consensus error: {}", msg),
            ClusterError::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
            ClusterError::TimeError(msg) => write!(f, "Time error: {}", msg),
            ClusterError::NodeNotFound(id) => write!(f, "Node not found: {}", id),
        }
    }
}

impl std::error::Error for ClusterError {}
