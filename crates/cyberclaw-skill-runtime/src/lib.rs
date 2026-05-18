//! cyberclaw-skill-runtime: Minimal skill runtime with in-memory registry.
//!
//! Provides the core abstractions for defining, registering, and retrieving
//! skill context. Skills provide methods, knowledge, and templates but do
//! **not** directly execute capabilities — all execution flows through
//! `Connector -> Capability`.
//!
//! ## Key types
//!
//! - [`SkillRuntime`] — core trait for skill registration and context retrieval
//! - [`SkillContext`] — context a skill provides to the agentic loop
//! - [`PluginRegistry`] — declarative plugin registration (replaces dynamic loading)

pub mod compat;
pub mod config;
pub mod context;
pub mod curator;
pub mod error;
pub mod handler;
pub mod loaders;
pub mod mock;
pub mod plugin_registry;
/// Sprint D4: PortabilityVerifier — static SKILL.md scan that emits a
/// portability-tier report (Tier1/Tier2/Tier3) with capability extraction.
pub mod portability_verifier;
pub mod runtime;
pub mod skill_executor;
pub mod skill_hub;
pub mod skill_packager;
pub mod skill_scanner;
pub mod skill_validator;
pub mod skills;
pub mod telemetry;

pub use compat::{detect_source_ecosystem, SkillCompat, SkillCompatRegistry, SourceEcosystem};
pub use config::SkillConfig;
pub use context::{SkillContext, SkillReference, ToolDeclaration};
pub use error::SkillRuntimeError;
pub use handler::SkillHandler;
pub use loaders::{
    ClaudeCodeSkillLoader, CodexSkillLoader, FormatLoader, HotReloadWatcher, LoadedSkill,
    OpenClawSkillLoader, SkillMetadata, SkillToolDeclaration, UnifiedSkillLoader,
};
pub use mock::MockSkillRuntime;
pub use plugin_registry::{PluginDeclaration, PluginRegistry};
pub use runtime::{MinimalSkillRuntime, SkillRuntime, SkillRuntimeConfig};
pub use skill_scanner::{
    ScanFinding, ScanResult, ScanSeverity, ScanVerdict, SkillScanner, SkillTrustLevel,
    ThreatCategory,
};
pub use skills::{CalculatorSkill, EchoSkill};

/// Re-export SkillId from core for convenience
pub use cyberclaw_core::ids::SkillId;

pub mod prelude {
    pub use crate::config::SkillConfig;
    pub use crate::context::{SkillContext, SkillReference, ToolDeclaration};
    pub use crate::handler::SkillHandler;
    pub use crate::loaders::{
        ClaudeCodeSkillLoader, CodexSkillLoader, FormatLoader, HotReloadWatcher, LoadedSkill,
        OpenClawSkillLoader, SkillMetadata, SkillToolDeclaration, UnifiedSkillLoader,
    };
    pub use crate::mock::MockSkillRuntime;
    pub use crate::plugin_registry::{PluginDeclaration, PluginRegistry};
    pub use crate::runtime::{MinimalSkillRuntime, SkillRuntime, SkillRuntimeConfig};
    pub use crate::skill_scanner::{
        ScanFinding, ScanResult, ScanSeverity, ScanVerdict, SkillScanner, SkillTrustLevel,
        ThreatCategory,
    };
    pub use crate::skills::{CalculatorSkill, EchoSkill};
    pub use crate::SkillId;
    pub use crate::SkillRuntimeError;
}
