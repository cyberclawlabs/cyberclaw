use crate::capability::{CapabilityEffect, RiskLevel};
use crate::cluster::CapabilityPlacement;
use crate::errors::{CoreError, CoreResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PackageKind {
    Agent,
    Skill,
    Connector,
    PlatformPlugin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Compatibility {
    pub platform: String,
    #[serde(default)]
    pub runtime: Vec<String>,
    #[serde(default)]
    pub os: Vec<String>,
    #[serde(default)]
    pub arch: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Dependencies {
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub connectors: Vec<String>,
    #[serde(default)]
    pub plugins: Vec<String>,
    #[serde(default)]
    pub packages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Artifacts {
    pub entry: Option<String>,
    #[serde(default)]
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageRef {
    pub id: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentClass {
    Master,
    Business,
    SecuritySupervisor,
    SubagentTemplate,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SpawnPolicy {
    pub can_spawn: bool,
    pub max_depth: Option<u32>,
    #[serde(default)]
    pub allowed_children: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSpec {
    pub role: String,
    pub class: AgentClass,
    pub description: String,
    pub persona_file: Option<String>,
    pub policy_file: Option<String>,
    pub memory_file: Option<String>,
    pub system_prompt_file: String,
    #[serde(default)]
    pub default_skills: Vec<String>,
    #[serde(default)]
    pub default_connectors: Vec<String>,
    pub default_workflow: Option<String>,
    #[serde(default)]
    pub spawn_policy: SpawnPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SkillFormat {
    #[serde(rename = "claude-compatible")]
    ClaudeCompatible,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSpec {
    pub format: SkillFormat,
    pub entry_file: String,
    pub scripts_dir: Option<String>,
    pub references_dir: Option<String>,
    pub assets_dir: Option<String>,
    #[serde(default)]
    pub suggested_agents: Vec<String>,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    #[serde(default)]
    pub suggested_connectors: Vec<String>,
    #[serde(default)]
    pub workflow_templates: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectorRuntime {
    Native,
    Remote,
    Process,
    Container,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorAuth {
    #[serde(default)]
    pub modes: Vec<String>,
    #[serde(default)]
    pub secret_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityTimeouts {
    pub request_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityContract {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub input_schema: String,
    pub output_schema: String,
    pub risk: RiskLevel,
    pub effects: Vec<CapabilityEffect>,
    pub placement: Option<CapabilityPlacement>,
    #[serde(default)]
    pub timeouts: CapabilityTimeouts,
}

/// 2026-05-17 (v1.2.9): network-bound capability prefixes. The dispatcher's
/// "Native runtime not allowed for High/Critical" hard gate is lifted for
/// these — their actual execution lives in an external process (browser via
/// CDP, MCP server, remote HTTP API), so cyberclaw's in-process Rust shim
/// can't be sandboxed by Container runtime without breaking the network
/// connection pool / session state. Real isolation is at the external
/// process boundary; governance review_threshold still gates by risk.
///
/// Prefixes are checked via `is_network_bound_capability(id)` below. This is
/// a hardcoded list (not a per-capability flag on `CapabilityContract`)
/// because it's a SMALL closed set of connector categories that all behave
/// the same way; threading a `network_bound: bool` field through 36+ struct
/// literals adds churn for zero behavioral difference.
pub const NETWORK_BOUND_PREFIXES: &[&str] = &["browser.", "mcp."];

/// Returns true when the capability ID indicates it bridges to an external
/// process (browser via CDP, MCP server). See `NETWORK_BOUND_PREFIXES` doc.
pub fn is_network_bound_capability(capability_id: &str) -> bool {
    NETWORK_BOUND_PREFIXES
        .iter()
        .any(|p| capability_id.starts_with(p))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorSpec {
    pub subtype: String,
    pub runtime: ConnectorRuntime,
    pub auth: Option<ConnectorAuth>,
    pub capabilities: Vec<CapabilityContract>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookPhase {
    Before,
    After,
    Around,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformHook {
    pub event: String,
    pub phase: HookPhase,
    pub handler: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginErrorPolicy {
    Continue,
    Fail,
    Disable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformPluginFailurePolicy {
    pub on_error: PluginErrorPolicy,
    pub emit_security_event: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformPluginSpec {
    pub runtime: String,
    pub hooks: Vec<PlatformHook>,
    #[serde(default)]
    pub permissions: BTreeMap<String, serde_json::Value>,
    pub failure_policy: PlatformPluginFailurePolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PackageSpec {
    Agent(AgentSpec),
    Skill(SkillSpec),
    Connector(ConnectorSpec),
    PlatformPlugin(PlatformPluginSpec),
}

impl PackageSpec {
    pub fn kind(&self) -> PackageKind {
        match self {
            Self::Agent(_) => PackageKind::Agent,
            Self::Skill(_) => PackageKind::Skill,
            Self::Connector(_) => PackageKind::Connector,
            Self::PlatformPlugin(_) => PackageKind::PlatformPlugin,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageManifest {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: PackageKind,
    pub id: String,
    pub version: String,
    pub name: String,
    pub display_name: Option<String>,
    pub summary: String,
    pub owner: Option<String>,
    pub license: Option<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub compatibility: Compatibility,
    #[serde(default)]
    pub dependencies: Dependencies,
    #[serde(default)]
    pub artifacts: Artifacts,
    pub config_schema: Option<String>,
    pub spec: PackageSpec,
}

impl PackageManifest {
    pub fn validate_spec_kind(&self) -> CoreResult<()> {
        if self.kind == self.spec.kind() {
            Ok(())
        } else {
            Err(CoreError::SchemaViolation(format!(
                "manifest kind {:?} does not match spec kind {:?}",
                self.kind,
                self.spec.kind()
            )))
        }
    }

    pub fn agent_spec(&self) -> Option<&AgentSpec> {
        match &self.spec {
            PackageSpec::Agent(spec) => Some(spec),
            _ => None,
        }
    }

    pub fn skill_spec(&self) -> Option<&SkillSpec> {
        match &self.spec {
            PackageSpec::Skill(spec) => Some(spec),
            _ => None,
        }
    }

    pub fn connector_spec(&self) -> Option<&ConnectorSpec> {
        match &self.spec {
            PackageSpec::Connector(spec) => Some(spec),
            _ => None,
        }
    }

    pub fn platform_plugin_spec(&self) -> Option<&PlatformPluginSpec> {
        match &self.spec {
            PackageSpec::PlatformPlugin(spec) => Some(spec),
            _ => None,
        }
    }
}
