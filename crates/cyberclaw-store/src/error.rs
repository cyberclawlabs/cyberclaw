//! Storage layer error type definitions.

/// Unified error type for the storage layer.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Database error (PostgreSQL via sqlx)
    #[cfg(feature = "postgres")]
    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),

    /// SQLite database error (via rusqlite)
    #[cfg(feature = "sqlite")]
    #[error("SQLite error: {0}")]
    SqliteError(#[from] rusqlite::Error),

    /// Record not found
    #[error("Not found: {0}")]
    NotFound(String),

    /// Serialization / deserialization error
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    /// Database connection error
    #[error("Connection error: {0}")]
    ConnectionError(String),

    /// Schema migration error
    #[error("Migration error: {0}")]
    MigrationError(String),

    /// Generic internal error
    #[error("Internal error: {0}")]
    InternalError(String),
}

/// Result type alias
pub type Result<T> = std::result::Result<T, StoreError>;
