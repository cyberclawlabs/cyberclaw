use cyberclaw_core::prelude::*;

#[derive(Debug, Clone)]
pub struct ControlPlaneContext {
    pub actor: ActorRef,
    pub session: Option<SessionRef>,
    pub workspace: Option<WorkspaceRef>,
}

#[derive(Debug, Clone)]
pub struct ResolutionInput {
    pub task: Task,
    pub case: Option<Case>,
    pub actor: ActorRef,
    pub workspace: Option<WorkspaceRef>,
    pub session: Option<SessionRef>,
    pub available_agents: Vec<AgentId>,
    pub available_skills: Vec<SkillId>,
    pub available_connectors: Vec<ConnectorId>,
    pub available_capabilities: Vec<CapabilityId>,
    pub available_workflows: Vec<WorkflowRef>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Resolution {
    pub agent: AgentId,
    pub skills: Vec<SkillId>,
    pub workflow: Option<WorkflowRef>,
    pub connectors: Vec<ConnectorId>,
    pub capabilities: Vec<CapabilityId>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlannedAction {
    pub connector_id: ConnectorId,
    pub capability: CapabilityId,
    pub input: serde_json::Value,
    pub reason: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExecutionPlan {
    pub resolution: Resolution,
    pub actions: Vec<PlannedAction>,
    pub review_required: bool,
    /// Sprint 10 (gradual landing): per-plan budget for the Autopilot Fix
    /// phase. Defaults to 5 when a plan is deserialized without this field
    /// (S10-T1+T2 backward compat). Drives the phase loop's "Fix → Execute"
    /// re-entry guard via [`drive_phase_loop_from_plan`].
    #[serde(default = "default_max_fix_loops")]
    pub max_fix_loops: u32,
    /// Sprint 10 (gradual landing): minimal evidence contract the Verify
    /// phase checks against — every entry must be satisfied by at least one
    /// [`cyberclaw_core::autopilot::ExecutionResult`] for the verifier to
    /// return `Pass`. Empty vec = no evidence contract → falls back to the
    /// legacy `AlwaysPass` behavior for backward compatibility with older
    /// plans (S27/S30 yaml never set this field).
    #[serde(default)]
    pub expected_outcomes: Vec<ExpectedOutcome>,
}

/// Default `max_fix_loops` for [`ExecutionPlan`] — used by serde and by
/// builders that don't set the field explicitly.
pub fn default_max_fix_loops() -> u32 {
    5
}

/// Sprint 10 minimal evidence contract: a single expectation the Verify phase
/// must observe in the collected [`ExecutionResult`]s. v1 supports the two
/// most useful predicates; richer matchers (JSON-path equality, error-pattern
/// matching, artifact-presence) are deferred until LLM-driven planners need
/// them.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ExpectedOutcome {
    /// At least one `result.output` (serialized to a JSON string) must
    /// contain this substring. Case-sensitive.
    OutputContains(String),
    /// At least one `result.status` must equal this string when formatted
    /// via `format!("{:?}", status).to_lowercase()`. Plain-string form so
    /// callers don't need to depend on the `cyberclaw_core::ExecutionStatus`
    /// enum directly when authoring plans (e.g. yaml admin UI).
    StatusEquals(String),
}

#[derive(Debug, Clone)]
pub enum PackageSource {
    LocalPath(String),
    Registry(String),
    Git(String),
    Archive(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryState {
    Discovered,
    Installed,
    Validated,
    Active,
    Disabled,
    Failed,
}

#[derive(Debug, Clone)]
pub struct PackageRecord {
    pub id: String,
    pub kind: PackageKind,
    pub latest_version: String,
    pub installed_versions: Vec<String>,
    pub active_version: Option<String>,
    pub source: PackageSource,
    pub state: RegistryState,
    pub available_nodes: Vec<NodeId>,
    pub runtime_requirements: Vec<String>,
    /// Full package manifest including capabilities and specs
    pub manifest: PackageManifest,
}

#[derive(Debug, Clone)]
pub struct LoadedAgent;

#[derive(Debug, Clone)]
pub struct LoadedSkill;

#[derive(Debug, Clone)]
pub struct LoadedConnector;

#[derive(Debug, Clone)]
pub struct LoadedPlatformPlugin;

#[derive(Debug, Clone)]
pub enum LoadedObject {
    Agent(LoadedAgent),
    Skill(LoadedSkill),
    Connector(LoadedConnector),
    PlatformPlugin(LoadedPlatformPlugin),
}

#[derive(Debug, Clone)]
pub struct LoadedPackage {
    pub manifest: PackageManifest,
    pub object: LoadedObject,
    pub source: PackageSource,
}

#[derive(Debug, Clone)]
pub enum LoadFailureKind {
    MissingManifest,
    SchemaViolation,
    MissingArtifact,
    InvalidDependency,
    InvalidHook,
    InvalidCapability,
    TrustValidationFailed,
    UnsupportedCompatibility,
}

// ============ Agent 6: Autopilot Support ============

/// Autopilot job metadata for autonomous multi-iteration execution
#[derive(Debug, Clone)]
pub struct AutopilotJob {
    pub job_id: String,
    pub goal: String,
    pub max_iterations: u32,
    pub review_gates: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Extended resolution result that may include autopilot-specific data
#[derive(Debug, Clone)]
pub struct ExtendedResolution {
    pub base: Resolution,
    pub autopilot_job: Option<AutopilotJob>,
    /// Sprint D3: when the request runs in `ExecutionMode::Persistent`, the
    /// resolver populates a Story DAG here. `None` for Normal/Autopilot
    /// requests so existing callers see no behavioural change.
    pub persistent_plan: Option<crate::persistent_execution::PersistentExecutionPlan>,
}
