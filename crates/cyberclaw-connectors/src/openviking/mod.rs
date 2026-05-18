//! OpenViking memory connector — CyberClaw's default external memory layer.
//!
//! Communicates with an independently deployed OpenViking instance via REST API
//! (AGPL-safe: no code linking, no Python SDK embedding).
//!
//! ## Phase A (current): 7 read-only capabilities
//!
//! | Capability | Risk | Description |
//! |---|---|---|
//! | `openviking.memory.ls` | Low | List memory namespace |
//! | `openviking.memory.tree` | Low | Tree view of memory hierarchy |
//! | `openviking.memory.read` | Low | Read resource at given depth |
//! | `openviking.memory.search` | Low | Semantic/keyword search |
//! | `openviking.memory.find` | Low | Pattern-based find |
//! | `openviking.memory.abstract` | Low | L0 abstract (~100 tokens) |
//! | `openviking.memory.overview` | Low | L1 overview (~2000 tokens) |
//!
//! ## L0/L1/L2 Semantic Reversal
//!
//! CyberClaw `MemoryLevel`: L0=Full, L1=Summary, L2=Metadata
//! OpenViking `ContextLevel`: L0=Abstract, L1=Overview, L2=Detail
//!
//! Use [`OvRetrievalDepth`] — never reuse `MemoryLevel` directly.

pub mod circuit_breaker;
pub mod connector;
pub mod health;
pub mod types;

pub use connector::OpenVikingConnector;
pub use types::{OpenVikingConfig, OvRetrievalDepth};
