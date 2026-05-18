use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConsensusError {
    #[error("Node is not the leader")]
    NotLeader,

    #[error("Proposal rejected: {0}")]
    ProposalRejected(String),

    #[error("Log compacted at index {0}")]
    LogCompacted(u64),

    #[error("Invalid term: expected {expected}, got {actual}")]
    InvalidTerm { expected: u64, actual: u64 },

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Timeout occurred: {0}")]
    Timeout(String),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Snapshot error: {0}")]
    SnapshotError(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("gRPC error: {0}")]
    Grpc(#[from] tonic::Status),
}

pub type Result<T> = std::result::Result<T, ConsensusError>;
