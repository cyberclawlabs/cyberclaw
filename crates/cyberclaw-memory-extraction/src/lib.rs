//! Cold-path memory extraction pipeline for CyberClaw.
//!
//! Inspired by EverOS memory concepts: MemCell boundary detection, Foresight prediction,
//! AtomicFact extraction, Episode extraction, and quality evaluation.
//!
//! All extraction is async and non-blocking. Never called on the critical execution path.

pub mod atomic_fact;
pub mod episode;
pub mod foresight;
pub mod llm_extractors;
pub mod memcell;
pub mod quality;
