//! Memory subsystem for CyberClaw.
//!
//! This module provides memory management capabilities for Working, Episodic, and Procedural memories.

pub mod compaction;
pub mod episodic;
pub mod procedural;
pub mod provider;
pub mod semantic;
pub mod working;

pub use compaction::{CompactionCheckpoint, CompactionResult, CompactionStrategy, SimpleCompactor};
pub use episodic::{EpisodicContextProvider, EpisodicProjection, EpisodicQuery, ScoredExecution};
pub use procedural::{ProceduralLoader, ProceduralRule};
pub use provider::{MemoryConfig, MemoryContextProvider};
pub use semantic::{
    MemoryDocument, MemoryDocumentKind, MemoryEntry, SemanticError, SemanticMemory, SemanticResult,
    SemanticSnapshot, UnknownMemoryDocumentKind, DEFAULT_AGENT_NOTES_CHAR_LIMIT,
    DEFAULT_USER_PROFILE_CHAR_LIMIT, ENTRY_DELIMITER,
};
pub use working::{
    InMemoryWorkingMemory, WorkingMemory, WorkingMemoryConfig, WorkingMemoryManager,
    WorkingMemoryRegistry,
};
