#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("invalid state transition: {0}")]
    InvalidStateTransition(String),
    #[error("invalid reference: {0}")]
    InvalidReference(String),
    #[error("schema violation: {0}")]
    SchemaViolation(String),
    #[error("governance denied: {0}")]
    GovernanceDenied(String),
}

pub type CoreResult<T> = Result<T, CoreError>;

/// Memory subsystem errors
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("lock poisoned: {0}")]
    LockPoisoned(String),
    #[error("capacity exceeded: max={max}, current={current}")]
    CapacityExceeded { max: usize, current: usize },
    #[error("operation failed: {0}")]
    OperationFailed(String),
}

pub type MemoryResult<T> = Result<T, MemoryError>;
