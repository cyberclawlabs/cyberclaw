//! Tool description facade — re-export from `cyberclaw-core`.
//!
//! `CapabilityFacade` was relocated to `cyberclaw-core::facade` on
//! 2026-05-06 (F8 architecture review). It lives there now so connectors
//! can declare their own facades without reverse-importing this crate
//! (which would create a `connectors → agent-runtime` dependency cycle —
//! the inverse of the actual `agent-runtime --features tool-calling →
//! connectors` direction).
//!
//! This module is kept as a thin re-export to preserve every existing
//! `cyberclaw_agent_runtime::tool_description::CapabilityFacade` import
//! path. New code should prefer `cyberclaw_core::facade::CapabilityFacade`
//! directly.
//!
//! See `crates/cyberclaw-core/src/facade.rs` for the type, factory
//! constructor, and the Anthropic / OpenAI tool-format projections.

pub use cyberclaw_core::facade::CapabilityFacade;
