//! CyberClaw platform persistent storage layer.
//!
//! Provides a unified state store abstraction supporting multiple backends:
//! - PostgreSQL (production)
//! - SQLite (lightweight persistent)
//! - In-memory store (testing and development)
//!
//! # Example
//!
//! ```rust,no_run
//! use cyberclaw_store::{StateStore, InMemoryStateStore, ExecutionRecord};
//! use uuid::Uuid;
//! use chrono::Utc;
//! use serde_json::json;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let store = InMemoryStateStore::new();
//!
//!     let record = ExecutionRecord {
//!         id: Uuid::new_v4(),
//!         agent_id: "my-agent".to_string(),
//!         skill_id: None,
//!         status: "running".to_string(),
//!         input: json!({"task": "example"}),
//!         output: None,
//!         error: None,
//!         started_at: Utc::now(),
//!         completed_at: None,
//!     };
//!
//!     store.save_execution(record).await?;
//!
//! #   Ok(())
//! # }
//! ```

pub mod conversation_session;
pub mod error;
pub mod memory;
pub mod memory_store;
pub mod migration;
pub mod scoped_memory;
pub mod semantic_memory;
pub mod skill_archive_repository;
pub mod state_store;

#[cfg(feature = "sqlite")]
pub mod sqlite;

pub use conversation_session::{
    ConversationId, ConversationSession, InMemorySessionStore, SessionStore, SessionSummary,
};
pub use error::{Result, StoreError};
pub use memory::InMemoryStateStore;
pub use memory_store::{
    InMemoryLeveledStore, LeveledMemoryRecord, LeveledMemoryStore, MemoryLevel, MemoryReadRecord,
};

#[cfg(feature = "sqlite")]
pub use memory_store::sqlite_leveled::SqliteLeveledStore;
pub use migration::{create_store, MigrationRunner, MigrationVersion, StoreBackend};
pub use scoped_memory::{CompressedTurn, MessageBlock, ScopedMemory, Turn, TurnRole};
pub use semantic_memory::{
    InMemorySemanticMemoryStore, MemoryKind, MemoryProvenance, MemoryScope, ProceduralRule,
    SemanticMemoryEntry, SemanticMemoryStore,
};
pub use skill_archive_repository::{
    InMemorySkillArchiveRepository, SkillArchiveRepository, SkillVariantRecord,
};
pub use state_store::{
    ArtifactRecord, AuditLogRecord, ExecutionRecord, JournalRecord, PolicyRecord, StateStore,
    TraceRecord,
};

#[cfg(feature = "postgres")]
pub use state_store::{PostgresConfig, PostgresMemoryStore, PostgresStateStore};

#[cfg(feature = "sqlite")]
pub use semantic_memory::SqliteSemanticMemoryStore;

#[cfg(feature = "sqlite")]
pub use skill_archive_repository::SqliteSkillArchiveRepository;

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStateStore;
