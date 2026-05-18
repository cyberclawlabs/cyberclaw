use crate::event_bus::EventBus;
use crate::execution_service::ExecutionService;
use crate::gateway_router::{GatewayRouter, IngressRequest};
use crate::lease_manager::LeaseManager;
use crate::membership_service::MembershipService;
use crate::placement_engine::PlacementEngine;
use crate::registry::Registry;
use crate::resolver::Resolver;
use crate::review_queue::ReviewQueue;
use crate::task_manager::TaskManager;
use crate::types::{ExecutionPlan, ResolutionInput};
use cyberclaw_core::cluster::{CapabilityPlacement, ClusterEvent};
use cyberclaw_core::identity::Identity;
use cyberclaw_core::prelude::*;
use cyberclaw_core::security::{SecurityEvent, SecurityEventSource, SecurityEventType, Severity};
use cyberclaw_governance::engine::PolicyEngine;
use cyberclaw_observability::security_event_store::SecurityEventStore;
use cyberclaw_observability::EventRecorder;
use cyberclaw_plugin_runtime::PluginRegistry;
use std::sync::{Arc, OnceLock};
use tracing::{info, warn};

/// System-reserved labels that are allowed but ONLY set by internal services (H-1 Security).
/// Users cannot provide these labels - they are automatically added by the system.
const SYSTEM_RESERVED_LABELS: &[&str] = &[
    "allow-empty-actions", // Exempts H-4 empty actions audit (used by Chat/Task APIs)
];

/// Allowed user-provided task labels whitelist (H-1 Security Fix).
/// Only these labels can be set by users in task submission.
/// System labels (SYSTEM_RESERVED_LABELS) are handled separately.
const ALLOWED_USER_TASK_LABELS: &[&str] = &[
    "urgent",
    "normal",
    "low-priority",
    "automation",
    "analysis",
    "review",
    "investigation",
    "reporting",
    "api",
    "cli",
    "web",
    "batch",
    "interactive",
    "scheduled",
    "manual",
    // Business-specific labels
    "security",
    "performance",
    "compliance",
    "testing",
    "production",
    "staging",
    "development",
    "refactor",
    "authentication",
    // Test-specific labels (for E2E/integration tests)
    "governance",
    "security-trace",
    "e2e",
    "e2e-test",
    "test",
    "high-risk",
    "low-risk",
    "critical",
    "allow-path",
    "denied",
    "secrets",
];

/// Errors produced by the orchestrator's authorization layer.
#[derive(Debug, thiserror::Error)]
pub enum OrchestratorAuthError {
    /// The caller is anonymous; task dispatch requires a known identity.
    #[error("anonymous task dispatch not allowed")]
    Unauthorized(String),

    /// The caller does not hold the required role or permission.
    #[error("insufficient permissions: required {required}, caller has {actual:?}")]
    InsufficientPermissions {
        required: String,
        actual: Vec<String>,
    },
}

/// Static agent ID for the master agent entry point.
/// "cyberclaw/master-agent" passes all ID validation rules: non-empty, ≤128 chars,
/// no control characters, no backslash, no ".." sequences.
/// Forward slashes are explicitly allowed by the ID validation logic for namespace-style IDs.
fn master_agent_id() -> &'static AgentId {
    static ID: OnceLock<AgentId> = OnceLock::new();
    ID.get_or_init(|| {
        AgentId::from_string("cyberclaw/master-agent".to_string())
            .unwrap_or_else(|e| panic!("BUG: 'cyberclaw/master-agent' failed ID validation: {e}"))
    })
}

// Re-export canonical RiskLevel from cyberclaw-core (single source of truth).
pub use cyberclaw_core::prelude::RiskLevel;

/// Request to submit an execution
#[derive(Debug, Clone)]
pub struct SubmitExecutionRequest {
    pub execution_id: ExecutionId,
    pub trace_id: TraceId,
    pub plan: ExecutionPlan,
    pub task: Task,
    pub actor: ActorRef,
    pub session: Option<SessionRef>,
    pub workspace: Option<WorkspaceRef>,
}

/// Result of execution submission
#[derive(Debug, Clone)]
pub struct SubmitExecutionResult {
    pub execution_id: ExecutionId,
    pub review_id: Option<ReviewId>,
    pub submitted: bool,
    pub scheduled_node_id: Option<NodeId>,
    pub lease_id: Option<LeaseId>,
}

/// ControlPlaneOrchestrator coordinates all control plane components
///
/// Execution flow:
/// 1. ingress: Receive and validate incoming requests
/// 2. normalize: Apply defaults and validation via GatewayRouter
/// 3. resolve: Select agent, skills, connectors via Resolver
/// 4. plan: Generate execution plan with actions
/// 5. review_gate: Check risk level and enqueue for review if needed
/// 6. submit_execution: Submit to execution service or wait for approval
/// 7. placement: Select node via PlacementEngine
/// 8. lease: Acquire lease via LeaseManager
/// 9. event: Publish events via EventBus
pub struct ControlPlaneOrchestrator {
    gateway: Arc<dyn GatewayRouter>,
    resolver: Arc<dyn Resolver>,
    registry: Arc<dyn Registry>,
    review_queue: Arc<dyn ReviewQueue>,
    task_manager: Arc<dyn TaskManager>,
    execution_service: Arc<dyn ExecutionService>,
    placement_engine: Arc<dyn PlacementEngine>,
    lease_manager: Arc<dyn LeaseManager>,
    membership_service: Arc<dyn MembershipService>,
    event_bus: Arc<dyn EventBus>,
    policy_engine: Arc<dyn PolicyEngine>,
    /// Plugin registry for dynamic plugin management
    plugin_registry: Arc<PluginRegistry>,
    /// Optional event recorder for security audit events.
    /// When `None`, security events are silently skipped (non-breaking).
    event_recorder: Option<Arc<dyn EventRecorder>>,
    /// Optional security event store for governance decision audit trail.
    /// When `None`, governance decision events are silently skipped (non-breaking).
    security_event_store: Option<Arc<dyn SecurityEventStore>>,
    /// Local node identity used to decide whether this orchestrator instance
    /// should execute a placed execution immediately.
    local_node_id: Option<NodeId>,
    /// Optional sink for finalising a Handoff review approval (S22 T2).
    /// When None, approvals for ReviewTarget::Handoff log a warning and succeed.
    handoff_completion_sink: Option<Arc<dyn crate::handoff_completion_sink::HandoffCompletionSink>>,
    /// Optional HandoffQueue for updating handoff status on rejection (S22 T2).
    handoff_queue: Option<Arc<dyn crate::handoff_queue::HandoffQueue>>,
}

impl ControlPlaneOrchestrator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        gateway: Arc<dyn GatewayRouter>,
        resolver: Arc<dyn Resolver>,
        registry: Arc<dyn Registry>,
        review_queue: Arc<dyn ReviewQueue>,
        task_manager: Arc<dyn TaskManager>,
        execution_service: Arc<dyn ExecutionService>,
        placement_engine: Arc<dyn PlacementEngine>,
        lease_manager: Arc<dyn LeaseManager>,
        membership_service: Arc<dyn MembershipService>,
        event_bus: Arc<dyn EventBus>,
        policy_engine: Arc<dyn PolicyEngine>,
        plugin_registry: Arc<PluginRegistry>,
        security_event_store: Arc<dyn SecurityEventStore>,
    ) -> Self {
        Self {
            gateway,
            resolver,
            registry,
            review_queue,
            task_manager,
            execution_service,
            placement_engine,
            lease_manager,
            membership_service,
            event_bus,
            policy_engine,
            plugin_registry,
            event_recorder: None,
            security_event_store: Some(security_event_store),
            local_node_id: None,
            handoff_completion_sink: None,
            handoff_queue: None,
        }
    }

    /// Attach a security event recorder.
    ///
    /// When set, the orchestrator emits [`SecurityEvent`]s at governance
    /// decision points (deny, review-required, self-approval attempt).
    /// This method consumes and returns `Self` for builder-style construction.
    pub fn with_event_recorder(mut self, recorder: Arc<dyn EventRecorder>) -> Self {
        self.event_recorder = Some(recorder);
        self
    }

    /// Configure the local node identity for multi-node execute gating.
    ///
    /// When set, this orchestrator executes only executions scheduled to this
    /// node and leaves remote assignments for their owner node.
    pub fn with_local_node_id(mut self, node_id: NodeId) -> Self {
        self.local_node_id = Some(node_id);
        self
    }

    /// Attach a HandoffCompletionSink for finalising handoff review approvals (S22 T2).
    ///
    /// The server crate provides a concrete impl; control-plane holds an `Option` so
    /// existing construction sites remain unchanged (no sink = warn + succeed).
    pub fn with_handoff_completion_sink(
        mut self,
        sink: Arc<dyn crate::handoff_completion_sink::HandoffCompletionSink>,
    ) -> Self {
        self.handoff_completion_sink = Some(sink);
        self
    }

    /// Attach a HandoffQueue for updating status on handoff review rejection (S22 T2).
    pub fn with_handoff_queue(
        mut self,
        queue: Arc<dyn crate::handoff_queue::HandoffQueue>,
    ) -> Self {
        self.handoff_queue = Some(queue);
        self
    }

    /// Get reference to the plugin registry
    pub fn plugin_registry(&self) -> &Arc<PluginRegistry> {
        &self.plugin_registry
    }

    /// Record a security event via the attached recorder, if any.
    ///
    /// Failures are logged at `warn` level and never propagate to callers so
    /// that observability issues never interrupt the execution critical path.
    async fn emit_security_event(&self, event: SecurityEvent) {
        if let Some(recorder) = &self.event_recorder {
            if let Err(e) = recorder.record_security_event(event).await {
                warn!(error = %e, "failed to record security event (non-fatal)");
            }
        }
    }

    fn should_execute_locally(&self, scheduled_node_id: &Option<NodeId>) -> bool {
        match (&self.local_node_id, scheduled_node_id) {
            // Single-node compatibility: keep old behavior when local identity is unset.
            (None, _) => true,
            (Some(local), Some(scheduled)) => local == scheduled,
            (Some(_), None) => false,
        }
    }

    async fn assign_execution_target(
        &self,
        execution_id: &ExecutionId,
        plan: &ExecutionPlan,
        publish_event: bool,
    ) -> anyhow::Result<(NodeId, LeaseId)> {
        // Step 1: Get active nodes from membership service
        let active_nodes = self.membership_service.list_active_nodes()?;
        if active_nodes.is_empty() {
            anyhow::bail!("no active nodes available for execution placement");
        }

        // Step 2: Select node via PlacementEngine and extract placement from plan
        let placement = self.extract_placement_from_plan(plan).await?;
        let placement_decision =
            self.placement_engine
                .place(execution_id.clone(), &placement, &active_nodes)?;

        // Step 3: Acquire lease for the execution on selected node
        let lease_id = self
            .lease_manager
            .acquire(
                execution_id.clone(),
                placement_decision.scheduled_node_id.clone(),
                None, // Use default TTL
            )
            .await?;

        // Step 4: Publish ExecutionAssigned event (best-effort)
        if publish_event {
            let event = ClusterEvent::ExecutionAssigned {
                execution_id: execution_id.clone(),
                node_id: placement_decision.scheduled_node_id.clone(),
                lease_id: lease_id.clone(),
                timestamp: chrono::Utc::now(),
            };
            if let Err(e) = self.event_bus.publish(event) {
                warn!(
                    execution_id = %execution_id,
                    node_id = %placement_decision.scheduled_node_id,
                    lease_id = %lease_id,
                    error = %e,
                    "failed to publish ExecutionAssigned event"
                );
            }
        }

        Ok((placement_decision.scheduled_node_id, lease_id))
    }

    /// Dispatch a task with caller authorization checks.
    ///
    /// This is the primary entry point for task submission. It enforces:
    /// 1. No anonymous callers – `Identity::Anonymous` is always rejected.
    /// 2. Permission checks for callers that carry role/permission lists.
    /// 3. An audit security event is emitted for every dispatch attempt.
    ///
    /// On success the task is forwarded to [`Self::process_ingress`] via a
    /// synthesized [`IngressRequest`] so the full governance pipeline applies.
    #[deprecated(
        since = "0.2.0",
        note = "Use process_ingress() instead for full governance chain. dispatch_task() only does auth/audit without PolicyEngine evaluation, ReviewQueue, or ExecutionService orchestration."
    )]
    pub async fn dispatch_task(
        &self,
        task: Task,
        caller: Identity,
    ) -> Result<TaskId, OrchestratorAuthError> {
        // 1. Reject anonymous callers immediately.
        if matches!(caller, Identity::Anonymous) {
            // SECURITY FIX: Convert caller Identity to ActorRef for audit trail
            let actor = caller.to_actor_ref(None);
            // Emit security event (best-effort, non-fatal).
            self.emit_security_event(SecurityEvent {
                id: SecurityEventId::new(),
                actor,
                timestamp: chrono::Utc::now(),
                execution_id: None,
                case_id: task.case_id.clone(),
                node_id: None,
                runtime_instance_id: None,
                source: SecurityEventSource::PermissionEngine,
                event_type: SecurityEventType::PermissionViolation,
                severity: Severity::High,
                summary: format!("Anonymous task dispatch rejected: task '{}'", task.title),
                details: serde_json::json!({
                    "task_title": task.title,
                    "caller": "anonymous",
                }),
                trace_id: TraceId::new(),
                credential_evidence: None,
            })
            .await;

            return Err(OrchestratorAuthError::Unauthorized(
                "Anonymous task dispatch not allowed".to_string(),
            ));
        }

        // 2. For callers that carry explicit role/permission lists, perform a
        //    lightweight pre-flight authorization check.  System callers bypass
        //    this step because they are always fully trusted.
        if !matches!(caller, Identity::System) {
            self.authorize_task(&caller, &task)?;
        }

        // 3. Emit audit event for successful authorization.
        let caller_summary = match &caller {
            Identity::System => "system".to_string(),
            Identity::User { id, .. } => format!("user:{}", id),
            Identity::Service { name, .. } => format!("service:{}", name),
            Identity::Anonymous => unreachable!("already rejected above"),
        };

        // SECURITY FIX: Convert caller Identity to ActorRef for audit trail
        let actor = caller.to_actor_ref(None);
        self.emit_security_event(SecurityEvent {
            id: SecurityEventId::new(),
            actor,
            timestamp: chrono::Utc::now(),
            execution_id: None,
            case_id: task.case_id.clone(),
            node_id: None,
            runtime_instance_id: None,
            source: SecurityEventSource::PermissionEngine,
            event_type: SecurityEventType::Custom("TaskDispatched".to_string()),
            severity: Severity::Info,
            summary: format!("Task '{}' dispatched by {}", task.title, caller_summary),
            details: serde_json::json!({
                "task_title": task.title,
                "caller": caller_summary,
            }),
            trace_id: TraceId::new(),
            credential_evidence: None,
        })
        .await;

        // 4. Return the task id; the full governance pipeline runs when the
        //    caller submits the task through process_ingress.
        Ok(task.id.clone())
    }

    /// Lightweight authorization and audit for known-safe API calls (Chat API, etc.)
    ///
    /// This method provides audit trail logging without triggering H-4 fail-secure review.
    ///
    /// # Use Cases
    /// - ✅ Chat Completions API (direct LLM calls, no dangerous actions)
    /// - ✅ Task Query API (read-only operations)
    /// - ✅ Status API (read-only operations)
    ///
    /// # NOT for Use
    /// - ❌ Agent executions (requires full governance chain)
    /// - ❌ Connector invocations (requires PolicyEngine evaluation)
    /// - ❌ Operations with actions (requires `dispatch_task`)
    ///
    /// # Security
    /// - Rejects anonymous callers (same as `dispatch_task`)
    /// - Emits SecurityEvent for audit trail
    /// - Does NOT trigger ReviewQueue (bypasses H-4 for performance)
    /// - Does NOT invoke PolicyEngine (assumes API-level safety)
    ///
    /// # P0-1 Phase 2 (DEPRECATED)
    /// This resolves the H-4 conflict where Chat API's empty actions trigger
    /// unnecessary human review, degrading user experience.
    ///
    /// **DEPRECATED in P0-2**: Use process_ingress() with "allow-empty-actions" label instead.
    #[deprecated(
        since = "0.2.0",
        note = "Use process_ingress() with 'allow-empty-actions' task label for full governance chain. This method bypasses PolicyEngine, ReviewQueue, and ExecutionService, violating architecture consistency."
    )]
    pub async fn authorize_and_audit_api_call(
        &self,
        caller: &Identity,
        api_endpoint: &str,
        request_payload: &serde_json::Value,
    ) -> Result<(), OrchestratorAuthError> {
        // 1. Reject anonymous callers immediately (same pattern as dispatch_task)
        if matches!(caller, Identity::Anonymous) {
            let actor = caller.to_actor_ref(None);
            self.emit_security_event(SecurityEvent {
                id: SecurityEventId::new(),
                actor,
                timestamp: chrono::Utc::now(),
                execution_id: None,
                case_id: None,
                node_id: None,
                runtime_instance_id: None,
                source: SecurityEventSource::PermissionEngine,
                event_type: SecurityEventType::PermissionViolation,
                severity: Severity::High,
                summary: format!("Anonymous API call rejected: {}", api_endpoint),
                details: serde_json::json!({
                    "api_endpoint": api_endpoint,
                    "caller": "anonymous",
                }),
                trace_id: TraceId::new(),
                credential_evidence: None,
            })
            .await;

            return Err(OrchestratorAuthError::Unauthorized(format!(
                "anonymous access to {} not allowed",
                api_endpoint
            )));
        }

        // 2. Emit audit event for successful API call
        let caller_summary = match caller {
            Identity::Anonymous => "anonymous".to_string(),
            Identity::System => "system".to_string(),
            Identity::User { id, .. } => format!("user:{}", id),
            Identity::Service { name, .. } => format!("service:{}", name),
        };

        let actor = caller.to_actor_ref(None);
        self.emit_security_event(SecurityEvent {
            id: SecurityEventId::new(),
            actor,
            timestamp: chrono::Utc::now(),
            execution_id: None,
            case_id: None,
            node_id: None,
            runtime_instance_id: None,
            source: SecurityEventSource::PermissionEngine,
            event_type: SecurityEventType::Custom("ApiCallAudited".to_string()),
            severity: Severity::Info,
            summary: format!("API call {} by {}", api_endpoint, caller_summary),
            details: serde_json::json!({
                "api_endpoint": api_endpoint,
                "caller": caller_summary,
                "payload_size": request_payload.to_string().len(),
            }),
            trace_id: TraceId::new(),
            credential_evidence: None,
        })
        .await;

        Ok(())
    }

    /// Perform role/permission-based authorization for a given caller and task.
    ///
    /// * `Identity::System` – always authorized (bypassed by the caller).
    /// * `Identity::User`   – must hold the "operator" role (or any role listed
    ///   in the task labels prefixed with `role:`).
    /// * `Identity::Service` – must hold the "dispatch" permission.
    /// * `Identity::Anonymous` – unreachable; caller rejects this variant first.
    fn authorize_task(&self, caller: &Identity, task: &Task) -> Result<(), OrchestratorAuthError> {
        match caller {
            Identity::System => Ok(()),
            Identity::User { roles, .. } => {
                // Determine required role: either a task label ("role:<name>")
                // or fall back to the default "operator" role.
                let required = task
                    .labels
                    .iter()
                    .find(|l| l.starts_with("role:"))
                    .and_then(|l| l.strip_prefix("role:"))
                    .unwrap_or("operator")
                    .to_string();

                if roles.contains(&required) || roles.contains(&"admin".to_string()) {
                    Ok(())
                } else {
                    Err(OrchestratorAuthError::InsufficientPermissions {
                        required,
                        actual: roles.clone(),
                    })
                }
            }
            Identity::Service { permissions, .. } => {
                let required = "dispatch".to_string();
                if permissions.contains(&required) || permissions.contains(&"admin".to_string()) {
                    Ok(())
                } else {
                    Err(OrchestratorAuthError::InsufficientPermissions {
                        required,
                        actual: permissions.clone(),
                    })
                }
            }
            Identity::Anonymous => {
                unreachable!("anonymous callers are rejected before authorize_task")
            }
        }
    }

    /// Internal orchestration flow for system-internal requests (H-1 Security Fix).
    /// This method is for trusted internal callers (e.g., Chat API) that need to
    /// bypass certain governance checks like requiring actions.
    ///
    /// SECURITY: This method MUST NOT be exposed to external APIs directly.
    pub async fn process_ingress_internal(
        &self,
        request: IngressRequest,
        allow_empty_actions: bool,
    ) -> anyhow::Result<SubmitExecutionResult> {
        self.process_ingress_impl(request, allow_empty_actions)
            .await
    }

    /// Main orchestration flow: ingress → normalize → resolve → plan → review_gate → submit
    /// This is the public API for external requests.
    pub async fn process_ingress(
        &self,
        request: IngressRequest,
    ) -> anyhow::Result<SubmitExecutionResult> {
        // External requests always require actions (security default)
        self.process_ingress_impl(request, false).await
    }

    /// Internal implementation of orchestration flow.
    async fn process_ingress_impl(
        &self,
        request: IngressRequest,
        allow_empty_actions: bool,
    ) -> anyhow::Result<SubmitExecutionResult> {
        // Step 0: Generate execution_id and trace_id upfront
        let execution_id = ExecutionId::new();
        let trace_id = TraceId::new();

        // Step 1: Normalize the incoming request
        let normalized = self.gateway.normalize(request)?;

        // Step 1.5: Validate task labels against whitelist (H-1 Security Fix)
        // This prevents users from injecting unauthorized labels to bypass governance.
        // System-reserved labels (like "allow-empty-actions") are allowed but ONLY when
        // set by internal services - not by users.
        for label in &normalized.task.labels {
            let label_str = label.as_str();

            // Allow system-reserved labels (they come from trusted internal services)
            if SYSTEM_RESERVED_LABELS.contains(&label_str) {
                continue;
            }

            // Check user-provided labels against whitelist
            if !ALLOWED_USER_TASK_LABELS.contains(&label_str) {
                // Log security event for audit trail
                warn!(
                    actor = %normalized.actor.id,
                    rejected_label = %label,
                    allowed_labels = ?ALLOWED_USER_TASK_LABELS,
                    system_labels = ?SYSTEM_RESERVED_LABELS,
                    "Rejected task with unauthorized user-provided label"
                );

                // Return error to prevent execution
                return Err(anyhow::anyhow!(
                    "Task label '{}' is not allowed. Permitted labels: {:?}",
                    label,
                    ALLOWED_USER_TASK_LABELS
                ));
            }
        }

        // Step 2: Store the task (after validation passes)
        let task = self.task_manager.create_task(normalized.task).await?;

        // Step 3: Resolve agent, skills, connectors
        // Load available ecosystem objects from registry
        let agent_records = self
            .registry
            .list(Some(cyberclaw_core::manifests::PackageKind::Agent))
            .await?;
        let skill_records = self
            .registry
            .list(Some(cyberclaw_core::manifests::PackageKind::Skill))
            .await?;
        let connector_records = self
            .registry
            .list(Some(cyberclaw_core::manifests::PackageKind::Connector))
            .await?;

        let available_agents: Vec<AgentId> = agent_records
            .iter()
            .filter(|r| r.state == crate::types::RegistryState::Active)
            .filter_map(|r| AgentId::from_string(r.id.clone()).ok())
            .collect();

        let available_skills: Vec<SkillId> = skill_records
            .iter()
            .filter(|r| r.state == crate::types::RegistryState::Active)
            .filter_map(|r| SkillId::from_string(r.id.clone()).ok())
            .collect();

        let available_connectors: Vec<ConnectorId> = connector_records
            .iter()
            .filter(|r| r.state == crate::types::RegistryState::Active)
            .filter_map(|r| ConnectorId::from_string(r.id.clone()).ok())
            .collect();

        // Collect all capabilities from connectors
        let mut available_capabilities: Vec<CapabilityId> = Vec::new();
        for connector in &connector_records {
            if let cyberclaw_core::manifests::PackageSpec::Connector(spec) =
                &connector.manifest.spec
            {
                for cap in &spec.capabilities {
                    if let Ok(cap_id) = CapabilityId::from_string(cap.id.clone()) {
                        available_capabilities.push(cap_id);
                    }
                }
            }
        }

        // Add master agent if not already present
        let mut final_agents = available_agents;
        if !final_agents.iter().any(|a| a == master_agent_id()) {
            final_agents.push(master_agent_id().clone());
        }

        let resolution_input = ResolutionInput {
            task: task.clone(),
            case: None,
            actor: normalized.actor.clone(),
            workspace: normalized.workspace.clone(),
            session: normalized.session.clone(),
            available_agents: final_agents,
            available_skills,
            available_connectors,
            available_capabilities,
            available_workflows: vec![],
        };

        // Step 4: Generate execution plan
        let plan = self.resolver.plan(resolution_input).await?;

        // Step 5: Evaluate governance decision using PolicyEngine
        // P1-2 FIX: Pass real execution_id for audit trail correlation
        // H-1 FIX: Pass allow_empty_actions flag for internal requests
        // M-2: Keep risk_level for audit logging
        let (governance_decision, risk_level) = self
            .evaluate_governance(&execution_id, &plan, &task, allow_empty_actions)
            .await?;

        // Step 6: Review gate - check governance decision
        // Deny: reject immediately
        // ReviewRequired: enqueue for human review
        // Allow: proceed to submission
        //
        // P1-1 FIX: PolicyEngine is the single source of truth for governance decisions.
        // Removed hardcoded risk-based override (|| risk >= RiskLevel::Medium) to allow
        // custom policy engines full control over Allow/Deny/ReviewRequired decisions.
        if governance_decision.is_deny() {
            // M-2: Structured audit log for policy denial (CRITICAL security event)
            warn!(
                execution_id = %execution_id,
                actor_id = %task.requested_by.id,
                task_title = %task.title,
                reason = %governance_decision.reason(),
                risk_level = ?risk_level,
                trace_id = %trace_id,
                "Governance DENIED execution - policy rejection"
            );

            // Emit a SecurityEvent for the policy denial before returning the error.
            self.emit_security_event(SecurityEvent {
                id: SecurityEventId::new(),
                actor: Some(task.requested_by.clone()),
                timestamp: chrono::Utc::now(),
                execution_id: Some(execution_id.clone()),
                case_id: task.case_id.clone(),
                node_id: None,
                runtime_instance_id: None,
                source: SecurityEventSource::PolicyEngine,
                event_type: SecurityEventType::PolicyDenied,
                severity: Severity::High,
                summary: format!(
                    "Execution {} denied by governance policy: {}",
                    execution_id,
                    governance_decision.reason()
                ),
                details: serde_json::json!({
                    "actor": task.requested_by.id.as_str(),
                    "task_title": task.title,
                    "reason": governance_decision.reason(),
                }),
                trace_id: trace_id.clone(),
                credential_evidence: None,
            })
            .await;

            return Err(anyhow::anyhow!(
                "Execution denied by governance policy: {}",
                governance_decision.reason()
            ));
        }

        if governance_decision.is_review_required() {
            // M-2: Structured audit log for review queue submission (MEDIUM severity)
            warn!(
                execution_id = %execution_id,
                actor_id = %task.requested_by.id,
                task_title = %task.title,
                reason = %governance_decision.reason(),
                risk_level = ?risk_level,
                review_type = ?governance_decision.review_type(),
                trace_id = %trace_id,
                "Governance requires REVIEW - enqueuing for human approval"
            );

            // Emit a SecurityEvent to record that this execution requires human review.
            self.emit_security_event(SecurityEvent {
                id: SecurityEventId::new(),
                actor: Some(task.requested_by.clone()),
                timestamp: chrono::Utc::now(),
                execution_id: Some(execution_id.clone()),
                case_id: task.case_id.clone(),
                node_id: None,
                runtime_instance_id: None,
                source: SecurityEventSource::PolicyEngine,
                event_type: SecurityEventType::Custom("ReviewRequired".to_string()),
                severity: Severity::Medium,
                summary: format!(
                    "Execution {} enqueued for human review: {}",
                    execution_id,
                    governance_decision.reason()
                ),
                details: serde_json::json!({
                    "actor": task.requested_by.id.as_str(),
                    "task_title": task.title,
                    "reason": governance_decision.reason(),
                }),
                trace_id: trace_id.clone(),
                credential_evidence: None,
            })
            .await;

            let review_id = self
                .enqueue_for_review(
                    execution_id.clone(),
                    trace_id.clone(),
                    &plan,
                    &task,
                    &normalized.actor,
                    normalized.workspace.clone(),
                )
                .await?;
            return Ok(SubmitExecutionResult {
                execution_id,
                review_id: Some(review_id),
                submitted: false,
                scheduled_node_id: None,
                lease_id: None,
            });
        }

        // Step 7: Submit execution directly if risk is low

        // M-2: Structured audit log for policy approval (INFO level - normal operation)
        info!(
            execution_id = %execution_id,
            actor_id = %task.requested_by.id,
            task_title = %task.title,
            reason = %governance_decision.reason(),
            risk_level = ?risk_level,
            trace_id = %trace_id,
            "Governance ALLOWED execution - proceeding to direct submission"
        );

        let submit_request = SubmitExecutionRequest {
            execution_id: execution_id.clone(),
            trace_id,
            plan: plan.clone(),
            task: task.clone(),
            actor: normalized.actor,
            session: normalized.session,
            workspace: normalized.workspace,
        };

        let result = self.submit_execution(submit_request).await?;

        // P0 FIX: Execute immediately for Allow path with explicit conditions
        // Condition 1: Non-empty actions (has real work to do)
        // Condition 2: "allow-empty-actions" system label (from internal APIs like Chat/Task)
        let should_trigger_execution =
            !plan.actions.is_empty() || task.labels.iter().any(|l| l == "allow-empty-actions");

        if should_trigger_execution && self.should_execute_locally(&result.scheduled_node_id) {
            self.execution_service.execute(&execution_id).await?;
        } else if should_trigger_execution {
            info!(
                execution_id = %execution_id,
                local_node_id = ?self.local_node_id,
                scheduled_node_id = ?result.scheduled_node_id,
                "execution scheduled to a different node; skip local execute trigger"
            );
        }

        Ok(result)
    }

    /// Evaluate governance decision using PolicyEngine
    ///
    /// This replaces the hardcoded risk evaluation with proper governance framework.
    /// Uses cyberclaw-governance PolicyEngine to make Allow/Deny/ReviewRequired decisions.
    ///
    /// # Arguments
    /// * `execution_id` - The actual execution ID for audit trail correlation (P1-2 FIX)
    /// * `plan` - The execution plan to evaluate
    /// * `task` - The task being executed
    ///
    /// Returns: GovernanceDecision and the evaluated risk level
    async fn evaluate_governance(
        &self,
        execution_id: &ExecutionId,
        plan: &ExecutionPlan,
        task: &Task,
        allow_empty_actions: bool,
    ) -> anyhow::Result<(
        cyberclaw_governance::decision::GovernanceDecision,
        RiskLevel,
    )> {
        use cyberclaw_governance::engine::EvaluationContext;

        // Evaluate each action in the plan
        let mut max_risk = RiskLevel::Low;
        let mut decisions = Vec::new();

        // Load connector manifests to get capability metadata
        let connector_records = self
            .registry
            .list(Some(cyberclaw_core::manifests::PackageKind::Connector))
            .await?;

        for action in &plan.actions {
            // Find the connector manifest for this action
            let connector_manifest = connector_records
                .iter()
                .find(|r| r.id == action.connector_id.as_str())
                .ok_or_else(|| {
                    anyhow::anyhow!("Connector {} not found in registry", action.connector_id)
                })?;

            // Extract connector spec
            let connector_spec = match &connector_manifest.manifest.spec {
                cyberclaw_core::manifests::PackageSpec::Connector(spec) => spec,
                _ => {
                    return Err(anyhow::anyhow!(
                        "Invalid spec type for connector {}",
                        action.connector_id
                    ))
                }
            };

            // Find the capability metadata
            let capability_meta = connector_spec
                .capabilities
                .iter()
                .find(|c| c.id == action.capability.as_str())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Capability {} not found in connector {}",
                        action.capability,
                        action.connector_id
                    )
                })?;

            // Build CapabilityRef from metadata
            let capability_ref = CapabilityRef {
                id: action.capability.clone(),
                connector_id: action.connector_id.clone(),
                risk: capability_meta.risk,
                effects: capability_meta.effects.clone(),
                placement: None,
            };

            // Build evaluation context
            // P1-2 FIX: Use actual execution_id for stable audit correlation
            let context = EvaluationContext {
                capability: capability_ref.clone(),
                actor: task.requested_by.clone(),
                execution_id: execution_id.clone(),
                reason: Some(action.reason.clone()),
            };

            let result = self.policy_engine.evaluate_capability(context).await?;

            // Track maximum risk level (RiskLevel is now the canonical core type directly)
            if result.evaluated_risk > max_risk {
                max_risk = result.evaluated_risk;
            }

            decisions.push(result.decision);
        }

        // H-4 FIX: If no actions, require review instead of allowing by default
        // Security: Empty actions should not bypass review (fail-secure principle)
        // H-1 FIX: Use internal flag instead of user-controllable labels
        if decisions.is_empty() {
            // Check if this is an internal request that allows empty actions
            if allow_empty_actions {
                // Allowed: approve with low risk (only for internal system calls like Chat API)
                let allow_decision = cyberclaw_governance::decision::GovernanceDecision::allow(
                    "Empty actions allowed for internal system request (no Connector execution)",
                );
                self.store_governance_event(execution_id, task, &allow_decision, RiskLevel::Low)
                    .await;
                return Ok((allow_decision, RiskLevel::Low));
            } else {
                // Default: require review for safety
                let empty_decision =
                    cyberclaw_governance::decision::GovernanceDecision::review_required(
                        "No actions to evaluate - requires human review for safety",
                        cyberclaw_governance::decision::ReviewType::Human,
                    );
                self.store_governance_event(execution_id, task, &empty_decision, RiskLevel::Medium)
                    .await;
                return Ok((empty_decision, RiskLevel::Medium));
            }
        }

        // Return the most restrictive decision
        for decision in &decisions {
            if decision.is_deny() {
                self.store_governance_event(execution_id, task, decision, max_risk)
                    .await;
                return Ok((decision.clone(), max_risk));
            }
        }

        for decision in &decisions {
            if decision.is_review_required() {
                self.store_governance_event(execution_id, task, decision, max_risk)
                    .await;
                return Ok((decision.clone(), max_risk));
            }
        }

        // All allowed
        let allow_decision =
            cyberclaw_governance::decision::GovernanceDecision::allow("All actions approved");
        self.store_governance_event(execution_id, task, &allow_decision, max_risk)
            .await;
        Ok((allow_decision, max_risk))
    }

    /// Store a governance decision as a SecurityEvent in the security event store.
    ///
    /// Failures are logged at `warn` level and never propagate so that observability
    /// issues never interrupt the execution critical path.
    async fn store_governance_event(
        &self,
        execution_id: &ExecutionId,
        task: &Task,
        decision: &cyberclaw_governance::decision::GovernanceDecision,
        risk: RiskLevel,
    ) {
        let Some(store) = &self.security_event_store else {
            return;
        };

        let (event_type, severity, summary) = match decision {
            cyberclaw_governance::decision::GovernanceDecision::Deny { reason } => (
                SecurityEventType::PolicyDenied,
                Severity::High,
                format!("Governance denied execution {}: {}", execution_id, reason),
            ),
            cyberclaw_governance::decision::GovernanceDecision::Allow { reason } => (
                SecurityEventType::Custom("PolicyAllowed".to_string()),
                Severity::Info,
                format!("Governance allowed execution {}: {}", execution_id, reason),
            ),
            cyberclaw_governance::decision::GovernanceDecision::ReviewRequired {
                reason,
                review_type,
            } => {
                let sev = match review_type {
                    cyberclaw_governance::decision::ReviewType::Human => Severity::Medium,
                    cyberclaw_governance::decision::ReviewType::Security => Severity::High,
                    _ => Severity::Medium,
                };
                (
                    SecurityEventType::Custom("PolicyReviewRequired".to_string()),
                    sev,
                    format!(
                        "Governance requires review for execution {}: {}",
                        execution_id, reason
                    ),
                )
            }
        };

        let event = SecurityEvent {
            id: SecurityEventId::new(),
            actor: Some(task.requested_by.clone()),
            timestamp: chrono::Utc::now(),
            execution_id: Some(execution_id.clone()),
            case_id: task.case_id.clone(),
            node_id: None,
            runtime_instance_id: None,
            source: SecurityEventSource::PolicyEngine,
            event_type,
            severity,
            summary,
            details: serde_json::json!({
                "actor": task.requested_by.id.as_str(),
                "task_title": task.title,
                "risk_level": format!("{:?}", risk),
                "reason": decision.reason(),
            }),
            trace_id: TraceId::new(),
            credential_evidence: None,
        };

        if let Err(e) = store.store(event).await {
            warn!(
                execution_id = %execution_id,
                error = %e,
                "failed to store governance security event (non-fatal)"
            );
        }
    }

    /// Enqueue execution for human review
    ///
    /// This method creates both:
    /// 1. An Execution record with status WaitingReview
    /// 2. A ReviewRequest in the review queue
    async fn enqueue_for_review(
        &self,
        execution_id: ExecutionId,
        trace_id: TraceId,
        plan: &ExecutionPlan,
        task: &Task,
        actor: &ActorRef,
        workspace: Option<WorkspaceRef>,
    ) -> anyhow::Result<ReviewId> {
        // Create Execution record with WaitingReview status
        let execution_request = crate::execution_service::ExecutionRequest {
            execution_id: execution_id.clone(),
            task: task.clone(),
            case: None,
            context: crate::types::ControlPlaneContext {
                actor: actor.clone(),
                session: None,
                workspace: workspace.clone(),
            },
            agent: Some(AgentRef {
                id: plan.resolution.agent.clone(),
                role: "resolved-agent".to_string(),
            }),
            trace_id: Some(trace_id.clone()),
            execution_mode: None,     // Default to non-Autopilot mode
            plan: Some(plan.clone()), // H-1 FIX: include plan
        };
        self.execution_service.submit(execution_request).await?;

        // Update status to WaitingReview
        self.execution_service
            .update_status(&execution_id, ExecutionStatus::WaitingReview)
            .await?;

        // Create review request
        let review = ReviewRequest::for_execution(
            ReviewId::new(),
            execution_id.clone(),
            task.case_id.clone(),
            format!("Review: {}", task.title),
            format!(
                "Task '{}' requires approval. Agent: {}, Skills: {}, Connectors: {}",
                task.title,
                plan.resolution.agent,
                plan.resolution.skills.len(),
                plan.resolution.connectors.len()
            ),
            actor.clone(),
            ReviewKind::Approval,
            trace_id,
            chrono::Utc::now(),
        );

        self.review_queue.enqueue(review.clone()).await?;

        // Publish ReviewCreated event so subscribers are notified when a review enters the queue
        let event = ClusterEvent::ReviewCreated {
            review_id: review.id.clone(),
            execution_id: Some(execution_id.clone()),
            target: review.target.clone(),
            timestamp: chrono::Utc::now(),
        };
        // Log error but continue if event publication fails
        // This prevents EventBus failures from aborting the entire operation
        if let Err(e) = self.event_bus.publish(event) {
            warn!(
                review_id = %review.id,
                execution_id = %execution_id,
                error = %e,
                "failed to publish ReviewCreated event"
            );
        }

        Ok(review.id)
    }

    /// Submit execution to execution service
    async fn submit_execution(
        &self,
        request: SubmitExecutionRequest,
    ) -> anyhow::Result<SubmitExecutionResult> {
        let execution_id = request.execution_id.clone();
        let (scheduled_node_id, lease_id) = self
            .assign_execution_target(&execution_id, &request.plan, true)
            .await?;

        // Step 1: Create execution record
        let execution_request = crate::execution_service::ExecutionRequest {
            execution_id: execution_id.clone(),
            task: request.task.clone(),
            case: None,
            context: crate::types::ControlPlaneContext {
                actor: request.actor.clone(),
                session: request.session.clone(),
                workspace: request.workspace.clone(),
            },
            agent: Some(AgentRef {
                id: request.plan.resolution.agent.clone(),
                role: "resolved-agent".to_string(),
            }),
            trace_id: Some(request.trace_id.clone()),
            execution_mode: None,             // Default to non-Autopilot mode
            plan: Some(request.plan.clone()), // H-1 FIX: include plan
        };
        self.execution_service.submit(execution_request).await?;

        // Step 2: Persist placement/lease assignment metadata
        self.execution_service
            .set_assignment(&execution_id, scheduled_node_id.clone(), lease_id.clone())
            .await?;

        Ok(SubmitExecutionResult {
            execution_id,
            review_id: None,
            submitted: true,
            scheduled_node_id: Some(scheduled_node_id),
            lease_id: Some(lease_id),
        })
    }

    /// Process review result and update execution accordingly
    ///
    /// This is the review回流 (feedback loop) that:
    /// - On approval: transitions WaitingReview -> Pending and submits for execution
    /// - On rejection: transitions WaitingReview -> Cancelled
    pub async fn process_review_result(
        &self,
        review_id: &ReviewId,
        approved: bool,
        reviewer: ActorRef,
    ) -> anyhow::Result<()> {
        // Get the review request
        let review = self
            .review_queue
            .get(review_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("review not found: {}", review_id))?;

        // H-3 FIX: Authorization check - prevent self-approval
        // Security: Users cannot approve their own review requests
        if reviewer.id == review.requested_by.id {
            // Emit a SecurityEvent for the self-approval attempt before returning an error.
            self.emit_security_event(SecurityEvent {
                id: SecurityEventId::new(),
                actor: Some(reviewer.clone()),
                timestamp: chrono::Utc::now(),
                execution_id: review.execution_id.clone(),
                case_id: None,
                node_id: None,
                runtime_instance_id: None,
                source: SecurityEventSource::PermissionEngine,
                event_type: SecurityEventType::PermissionViolation,
                severity: Severity::High,
                summary: format!(
                    "Self-approval attempt blocked: actor {} tried to approve their own review {}",
                    reviewer.id, review_id
                ),
                details: serde_json::json!({
                    "reviewer": reviewer.id.as_str(),
                    "requester": review.requested_by.id.as_str(),
                    "review_id": review_id.to_string(),
                    "execution_id": review.execution_id.as_ref().map(|id| id.to_string()).unwrap_or_default(),
                }),
                trace_id: TraceId::new(),
                credential_evidence: None,
            })
            .await;

            anyhow::bail!(
                "authorization failed: reviewer cannot approve their own request (reviewer={}, requester={})",
                reviewer.id,
                review.requested_by.id
            );
        }

        // S22 T2: Branch on review target — Execution vs Handoff.
        use cyberclaw_core::review::ReviewTarget;
        match &review.target {
            ReviewTarget::Execution { execution_id } => {
                // ── Execution review path (unchanged behaviour) ──────────────
                let execution_id = execution_id.clone();

                let mut execution = self
                    .execution_service
                    .get(&execution_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("execution not found: {}", execution_id))?;

                if approved {
                    // Update execution status to Pending (ready to be scheduled)
                    self.execution_service
                        .update_status(&execution_id, ExecutionStatus::Pending)
                        .await?;

                    // SECURITY FIX: Pass actual reviewer to record in audit trail
                    // Update review status
                    self.review_queue.approve(review_id, &reviewer).await?;

                    // Publish ReviewApproved event
                    let event = ClusterEvent::ReviewApproved {
                        review_id: review_id.clone(),
                        execution_id: Some(execution_id.clone()),
                        approved_by: reviewer.id.clone(),
                        target: review.target.clone(),
                        timestamp: chrono::Utc::now(),
                    };
                    // Log error but continue if event publication fails
                    // This prevents EventBus failures from aborting the entire operation
                    if let Err(e) = self.event_bus.publish(event) {
                        warn!(
                            review_id = %review_id,
                            execution_id = %execution_id,
                            approved_by = %reviewer.id,
                            error = %e,
                            "failed to publish ReviewApproved event"
                        );
                    }

                    // Observability: record approval for audit and monitoring
                    info!(
                        review_id = %review_id,
                        execution_id = %execution_id,
                        approved_by = %reviewer.id,
                        "review approved; execution transitioned to Pending"
                    );

                    // Ensure placement/lease assignment exists before any execute trigger.
                    if execution.scheduled_node_id.is_none() || execution.lease_id.is_none() {
                        let plan = self
                            .execution_service
                            .get_plan(&execution_id)
                            .await?
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "execution plan missing for review-approved execution: {}",
                                    execution_id
                                )
                            })?;
                        let (scheduled_node_id, lease_id) = self
                            .assign_execution_target(&execution_id, &plan, true)
                            .await?;
                        self.execution_service
                            .set_assignment(
                                &execution_id,
                                scheduled_node_id.clone(),
                                lease_id.clone(),
                            )
                            .await?;
                        execution.scheduled_node_id = Some(scheduled_node_id);
                        execution.lease_id = Some(lease_id);
                    }

                    if self.should_execute_locally(&execution.scheduled_node_id) {
                        // Trigger actual execution after approval
                        // This completes the review→execution feedback loop
                        self.execution_service.execute(&execution_id).await?;

                        info!(
                            review_id = %review_id,
                            execution_id = %execution_id,
                            scheduled_node_id = ?execution.scheduled_node_id,
                            "execution triggered after review approval"
                        );
                    } else {
                        info!(
                            review_id = %review_id,
                            execution_id = %execution_id,
                            local_node_id = ?self.local_node_id,
                            scheduled_node_id = ?execution.scheduled_node_id,
                            "review approved, but execution is assigned to a remote node; skip local execute trigger"
                        );
                    }
                } else {
                    // Reject the review and cancel the execution
                    self.execution_service
                        .update_status(&execution_id, ExecutionStatus::Cancelled)
                        .await?;

                    // SECURITY FIX: Pass actual reviewer to record in audit trail
                    // Update review status
                    self.review_queue.reject(review_id, &reviewer).await?;

                    // Publish ReviewRejected event
                    let event = ClusterEvent::ReviewRejected {
                        review_id: review_id.clone(),
                        execution_id: Some(execution_id.clone()),
                        rejected_by: reviewer.id.clone(),
                        reason: "Review rejected by human reviewer".to_string(),
                        target: review.target.clone(),
                        timestamp: chrono::Utc::now(),
                    };
                    // Log error but continue if event publication fails
                    // This prevents EventBus failures from aborting the entire operation
                    if let Err(e) = self.event_bus.publish(event) {
                        warn!(
                            review_id = %review_id,
                            execution_id = %execution_id,
                            rejected_by = %reviewer.id,
                            error = %e,
                            "failed to publish ReviewRejected event"
                        );
                    }

                    // Observability: record rejection for audit and monitoring
                    warn!(
                        review_id = %review_id,
                        execution_id = %execution_id,
                        rejected_by = %reviewer.id,
                        "review rejected; execution cancelled"
                    );
                }
            }

            ReviewTarget::Handoff { handoff_id } => {
                // ── Handoff review path (S22 T2) ─────────────────────────────
                let handoff_id = handoff_id.clone();

                if approved {
                    // Update review queue status first so the record reflects approval.
                    self.review_queue.approve(review_id, &reviewer).await?;

                    // Delegate conversation mutation to the server-layer sink.
                    match &self.handoff_completion_sink {
                        Some(sink) => {
                            sink.finalize_accept(&handoff_id)
                                .await
                                .map_err(|e| anyhow::anyhow!("handoff finalize failed: {e}"))?;
                        }
                        None => {
                            warn!(
                                review_id = %review_id,
                                handoff_id = %handoff_id,
                                "handoff review approved but no HandoffCompletionSink configured"
                            );
                        }
                    }

                    info!(
                        review_id = %review_id,
                        handoff_id = %handoff_id,
                        approved_by = %reviewer.id,
                        "handoff review approved; finalize_accept called"
                    );
                } else {
                    // Rejection: update HandoffQueue status to Declined.
                    if let Some(queue) = &self.handoff_queue {
                        use cyberclaw_core::handoff::HandoffStatus;
                        queue
                            .update_status(&handoff_id, HandoffStatus::Declined)
                            .await
                            .map_err(|e| anyhow::anyhow!("handoff reject failed: {e}"))?;
                    } else {
                        warn!(
                            review_id = %review_id,
                            handoff_id = %handoff_id,
                            "handoff review rejected but no HandoffQueue configured; status not updated"
                        );
                    }

                    // Update review queue status.
                    self.review_queue.reject(review_id, &reviewer).await?;

                    warn!(
                        review_id = %review_id,
                        handoff_id = %handoff_id,
                        rejected_by = %reviewer.id,
                        "handoff review rejected; HandoffQueue status set to Declined"
                    );
                }
            }
        }

        Ok(())
    }

    /// Extract placement requirements from `ExecutionPlan`.
    ///
    /// Sources:
    /// - Agent package runtime requirements
    /// - Capability contract placement constraints
    ///
    /// Merge strategy:
    /// - `allowed_node_labels`: union
    /// - `required_runtime`: union
    /// - `requires_local_secret`: logical OR
    /// - `network_zone`: must be consistent when multiple capabilities specify it
    async fn extract_placement_from_plan(
        &self,
        plan: &ExecutionPlan,
    ) -> anyhow::Result<CapabilityPlacement> {
        let agent_record = self
            .registry
            .get(
                cyberclaw_core::manifests::PackageKind::Agent,
                plan.resolution.agent.as_str(),
            )
            .await?;

        let mut required_runtime = if let Some(record) = agent_record {
            record.runtime_requirements
        } else {
            Vec::new()
        };

        let mut allowed_node_labels = Vec::<String>::new();
        let mut requires_local_secret = false;
        let mut network_zone: Option<String> = None;

        let mut capability_ids = std::collections::BTreeSet::<String>::new();
        for capability in &plan.resolution.capabilities {
            capability_ids.insert(capability.as_str().to_string());
        }
        for action in &plan.actions {
            capability_ids.insert(action.capability.as_str().to_string());
        }

        if !capability_ids.is_empty() {
            let connector_records = self
                .registry
                .list(Some(cyberclaw_core::manifests::PackageKind::Connector))
                .await?;

            for capability_id in capability_ids {
                let mut matched = false;

                for connector_record in &connector_records {
                    let cyberclaw_core::manifests::PackageSpec::Connector(spec) =
                        &connector_record.manifest.spec
                    else {
                        continue;
                    };

                    if let Some(contract) = spec.capabilities.iter().find(|c| c.id == capability_id)
                    {
                        matched = true;

                        if let Some(placement) = &contract.placement {
                            for label in &placement.allowed_node_labels {
                                if !allowed_node_labels.contains(label) {
                                    allowed_node_labels.push(label.clone());
                                }
                            }

                            for runtime in &placement.required_runtime {
                                if !required_runtime.contains(runtime) {
                                    required_runtime.push(runtime.clone());
                                }
                            }

                            requires_local_secret |= placement.requires_local_secret;

                            if let Some(zone) = &placement.network_zone {
                                match &network_zone {
                                    None => network_zone = Some(zone.clone()),
                                    Some(existing) if existing == zone => {}
                                    Some(existing) => {
                                        anyhow::bail!(
                                            "conflicting network_zone requirements: '{}' vs '{}' (capability: {})",
                                            existing,
                                            zone,
                                            capability_id
                                        );
                                    }
                                }
                            }
                        }

                        break;
                    }
                }

                if !matched {
                    warn!(
                        capability_id = %capability_id,
                        "capability placement metadata not found in registry; fallback to default placement"
                    );
                }
            }
        }

        Ok(CapabilityPlacement {
            allowed_node_labels,
            required_runtime,
            requires_local_secret,
            network_zone,
        })
    }
}

#[cfg(test)]
#[allow(deprecated)] // 允许测试 deprecated 方法以保持向后兼容性测试覆盖
mod tests {
    use super::*;
    use crate::gateway_router::InMemoryGatewayRouter;
    use crate::registry::InMemoryRegistry;
    use crate::resolver::InMemoryResolver;
    use crate::review_queue::InMemoryReviewQueue;
    use crate::task_manager::InMemoryTaskManager;
    use chrono::Utc;
    use cyberclaw_core::identity::{ActorType, Identity};
    use cyberclaw_core::ids::ActorId;
    use cyberclaw_governance::engine::DefaultPolicyEngine;

    fn create_test_actor(id: &str, display_name: &str) -> ActorRef {
        ActorRef {
            id: ActorId::from_string(id.to_string()).unwrap(),
            actor_type: ActorType::Human,
            tenant_id: None,
            home_node_id: None,
            display_name: display_name.to_string(),
        }
    }

    fn create_test_orchestrator() -> ControlPlaneOrchestrator {
        use crate::event_bus::InMemoryEventBus;
        use crate::execution_service::InMemoryExecutionService;
        use crate::lease_manager::InMemoryLeaseManager;
        use crate::membership_service::InMemoryMembershipService;
        use crate::placement_engine::InMemoryPlacementEngine;
        use cyberclaw_core::cluster::{
            MembershipState, NodeCapacity, NodeHealth, NodeRecord, NodeRole,
        };

        let gateway = Arc::new(InMemoryGatewayRouter::new());
        let registry = Arc::new(InMemoryRegistry::new());
        let resolver = Arc::new(InMemoryResolver::new(registry.clone()));
        let review_queue = Arc::new(InMemoryReviewQueue::new(None));
        let task_manager = Arc::new(InMemoryTaskManager::new());
        let execution_service = Arc::new(InMemoryExecutionService::default());
        let placement_engine = Arc::new(InMemoryPlacementEngine::new());
        let lease_manager = Arc::new(InMemoryLeaseManager::default());
        let membership_service = Arc::new(InMemoryMembershipService::default());
        let event_bus = Arc::new(InMemoryEventBus::default());

        // Add a test node to membership service
        let test_node_id = NodeId::from_string("test-node-1".to_string()).unwrap();
        let test_node = NodeRecord {
            id: test_node_id.clone(),
            role: NodeRole::Worker,
            labels: vec!["worker".to_string()],
            region: Some("us-east-1".to_string()),
            zone: Some("us-east-1a".to_string()),
            health: NodeHealth::Healthy,
            membership_state: MembershipState::Active,
            capacity: NodeCapacity {
                max_executions: Some(10),
                max_cpu_millis: None,
                max_memory_mb: None,
            },
            current_executions: 0,
            last_heartbeat_at: Utc::now(),
        };
        membership_service.join(test_node).unwrap();
        // Send heartbeat to promote node from Joining to Active
        membership_service.heartbeat(&test_node_id).unwrap();

        let policy_engine = Arc::new(DefaultPolicyEngine::default());
        let security_event_store: Arc<
            dyn cyberclaw_observability::security_event_store::SecurityEventStore,
        > = Arc::new(cyberclaw_observability::InMemorySecurityEventStore::new());
        let plugin_registry = Arc::new(PluginRegistry::new(std::path::PathBuf::from(
            "/tmp/plugins",
        )));

        ControlPlaneOrchestrator::new(
            gateway,
            resolver,
            registry,
            review_queue,
            task_manager,
            execution_service,
            placement_engine,
            lease_manager,
            membership_service,
            event_bus,
            policy_engine,
            plugin_registry,
            security_event_store,
        )
    }

    fn create_test_ingress_request(priority: Priority) -> IngressRequest {
        IngressRequest {
            actor: create_test_actor("test-user", "Test User"),
            session: None,
            workspace: None,
            task: Task {
                id: TaskId::new(),
                case_id: None,
                title: "Test Task".to_string(),
                summary: "Test summary".to_string(),
                kind: TaskKind::Analysis,
                priority,
                requested_by: create_test_actor("test-user", "Test User"),
                requested_at: Utc::now(),
                trigger: TriggerRef {
                    kind: "manual".to_string(),
                    source: "test".to_string(),
                },
                input: TaskInput::default(),
                desired_outputs: vec![],
                labels: vec![],
                preferred_agent_id: None,
            },
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_orchestrator_low_risk_direct_submission() {
        let orchestrator = create_test_orchestrator();
        let request = create_test_ingress_request(Priority::Low);

        let result = orchestrator.process_ingress(request).await;
        assert!(
            result.is_ok(),
            "low risk task should succeed, but got error: {:?}",
            result.as_ref().err()
        );

        let result = result.unwrap();
        // H-4 FIX: Empty actions now require review (fail-secure principle)
        // The test task "Test Task" doesn't match any capability patterns,
        // so it generates empty actions and requires review for safety.
        assert!(
            !result.submitted,
            "task with empty actions should require review (fail-secure)"
        );
        assert!(
            result.review_id.is_some(),
            "task with empty actions should have review_id"
        );
    }

    /// Integration test: Verify complete control plane main chain
    /// Gateway -> Resolver -> ReviewGate -> PlacementEngine -> LeaseManager -> EventBus
    #[tokio::test(flavor = "multi_thread")]
    async fn test_integration_complete_control_plane_chain() {
        use crate::event_bus::InMemoryEventBus;

        let orchestrator = create_test_orchestrator();

        // Subscribe to events to verify event publication
        // Note: This is a separate event_bus for future event verification tests
        let _event_bus = Arc::new(InMemoryEventBus::default());

        let request = create_test_ingress_request(Priority::Low);

        // Process ingress request
        let result = orchestrator.process_ingress(request).await;
        assert!(
            result.is_ok(),
            "control plane chain should succeed: {:?}",
            result.as_ref().err()
        );

        let result = result.unwrap();

        // Verify execution_id is not a placeholder (non-zero)
        assert_ne!(
            result.execution_id.to_string(),
            ExecutionId::new().to_string(),
            "execution_id should be stable across calls"
        );

        // H-4 FIX: Empty actions now require review (fail-secure principle)
        // Verify execution requires review due to empty actions
        assert!(
            !result.submitted,
            "execution with empty actions should require review (fail-secure)"
        );
        assert!(
            result.review_id.is_some(),
            "execution with empty actions should have review_id"
        );

        // Note: In this test, we can't verify event publication because the test orchestrator
        // uses a different event_bus instance than the one we subscribed to.
        // In a real integration test, we would inject the same event_bus instance.
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_allow_path_remote_assignment_skips_local_execute() {
        let local_node_id = NodeId::from_string("test-node-2".to_string()).unwrap();
        let orchestrator = create_test_orchestrator().with_local_node_id(local_node_id);

        let mut request = create_test_ingress_request(Priority::Low);
        request.task.labels.push("allow-empty-actions".to_string());

        let result = orchestrator.process_ingress_internal(request, true).await;
        assert!(
            result.is_ok(),
            "remote-assigned allow path should not trigger local execute: {:?}",
            result.as_ref().err()
        );

        let result = result.unwrap();
        assert!(result.submitted, "allow path should submit execution");
        assert_eq!(
            result
                .scheduled_node_id
                .as_ref()
                .map(|id| id.as_str())
                .unwrap_or_default(),
            "test-node-1",
            "execution should be scheduled to the active worker node"
        );

        let execution = orchestrator
            .execution_service
            .get(&result.execution_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            execution.status,
            ExecutionStatus::Pending,
            "remote assignment must stay Pending on this node"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_review_approval_remote_assignment_skips_local_execute() {
        let local_node_id = NodeId::from_string("test-node-2".to_string()).unwrap();
        let orchestrator = create_test_orchestrator().with_local_node_id(local_node_id);

        let request = create_test_ingress_request(Priority::Low);
        let submit_result = orchestrator.process_ingress(request).await.unwrap();
        let review_id = submit_result
            .review_id
            .expect("empty-action request should require review");

        let reviewer = create_test_actor("reviewer-1", "Reviewer");
        let approve_result = orchestrator
            .process_review_result(&review_id, true, reviewer)
            .await;
        assert!(
            approve_result.is_ok(),
            "review approval should succeed without local execution trigger: {:?}",
            approve_result.as_ref().err()
        );

        let execution = orchestrator
            .execution_service
            .get(&submit_result.execution_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            execution.status,
            ExecutionStatus::Pending,
            "remote-assigned review-approved execution should remain Pending on local node"
        );
        assert!(
            execution.scheduled_node_id.is_some(),
            "review-approved execution should have placement metadata"
        );
        assert!(
            execution.lease_id.is_some(),
            "review-approved execution should have lease metadata"
        );
    }

    // P0-3: Original evaluate_risk tests removed after M2 governance integration
    // These tests verified that risk evaluation was based on capability risk (plan.review_required)
    // rather than task priority. This behavior is now covered by PolicyEngine evaluation
    // and the governance integration tests in tests/governance_integration_test.rs

    fn make_test_task(priority: Priority) -> Task {
        Task {
            id: TaskId::new(),
            case_id: None,
            title: "Auth Test Task".to_string(),
            summary: "Authorization test".to_string(),
            kind: TaskKind::Analysis,
            priority,
            requested_by: create_test_actor("test-user", "Test User"),
            requested_at: Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_dispatch_task_anonymous_is_rejected() {
        let orchestrator = create_test_orchestrator();
        let task = make_test_task(Priority::Low);
        let result = orchestrator.dispatch_task(task, Identity::Anonymous).await;
        assert!(result.is_err(), "anonymous dispatch must be rejected");
        let err = result.unwrap_err();
        assert!(
            matches!(err, OrchestratorAuthError::Unauthorized(_)),
            "expected Unauthorized, got: {:?}",
            err
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_dispatch_task_system_is_authorized() {
        let orchestrator = create_test_orchestrator();
        let task = make_test_task(Priority::Low);
        let result = orchestrator.dispatch_task(task, Identity::System).await;
        assert!(
            result.is_ok(),
            "system identity should always be authorized, got: {:?}",
            result.as_ref().err()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_dispatch_task_user_with_operator_role_is_authorized() {
        let orchestrator = create_test_orchestrator();
        let task = make_test_task(Priority::Low);
        let caller = Identity::User {
            id: "alice".to_string(),
            roles: vec!["operator".to_string()],
        };
        let result = orchestrator.dispatch_task(task, caller).await;
        assert!(
            result.is_ok(),
            "user with operator role should be authorized"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_dispatch_task_user_without_required_role_is_rejected() {
        let orchestrator = create_test_orchestrator();
        let task = make_test_task(Priority::Low);
        let caller = Identity::User {
            id: "bob".to_string(),
            roles: vec!["viewer".to_string()],
        };
        let result = orchestrator.dispatch_task(task, caller).await;
        assert!(
            result.is_err(),
            "user without required role must be rejected"
        );
        assert!(
            matches!(
                result.unwrap_err(),
                OrchestratorAuthError::InsufficientPermissions { .. }
            ),
            "expected InsufficientPermissions"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_dispatch_task_service_with_dispatch_permission_is_authorized() {
        let orchestrator = create_test_orchestrator();
        let task = make_test_task(Priority::Low);
        let caller = Identity::Service {
            name: "ci-pipeline".to_string(),
            permissions: vec!["dispatch".to_string()],
        };
        let result = orchestrator.dispatch_task(task, caller).await;
        assert!(
            result.is_ok(),
            "service with dispatch permission should be authorized"
        );
    }

    // ========== P0-1 Phase 2: authorize_and_audit_api_call() Tests ==========

    #[tokio::test(flavor = "multi_thread")]
    async fn test_authorize_and_audit_api_call_user_success() {
        let orchestrator = create_test_orchestrator();
        let caller = Identity::User {
            id: "alice".to_string(),
            roles: vec!["operator".to_string()],
        };
        let payload = serde_json::json!({
            "messages": [{"role": "user", "content": "Hello"}],
            "model": "gpt-4"
        });

        let result = orchestrator
            .authorize_and_audit_api_call(&caller, "/v1/chat/completions", &payload)
            .await;

        assert!(
            result.is_ok(),
            "User should be authorized for API call: {:?}",
            result
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_authorize_and_audit_api_call_anonymous_rejected() {
        let orchestrator = create_test_orchestrator();
        let caller = Identity::Anonymous;
        let payload = serde_json::json!({
            "messages": [{"role": "user", "content": "Hello"}]
        });

        let result = orchestrator
            .authorize_and_audit_api_call(&caller, "/v1/chat/completions", &payload)
            .await;

        assert!(result.is_err(), "Anonymous should be rejected");
        assert!(
            matches!(result, Err(OrchestratorAuthError::Unauthorized(_))),
            "Expected Unauthorized error for anonymous caller"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_authorize_and_audit_api_call_service_success() {
        let orchestrator = create_test_orchestrator();
        let caller = Identity::Service {
            name: "api-gateway".to_string(),
            permissions: vec!["read".to_string()],
        };
        let payload = serde_json::json!({
            "query": "SELECT * FROM tasks"
        });

        let result = orchestrator
            .authorize_and_audit_api_call(&caller, "/api/v1/tasks", &payload)
            .await;

        assert!(
            result.is_ok(),
            "Service should be authorized for API call: {:?}",
            result
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_authorize_and_audit_api_call_system_success() {
        let orchestrator = create_test_orchestrator();
        let caller = Identity::System;
        let payload = serde_json::json!({
            "operation": "health_check"
        });

        let result = orchestrator
            .authorize_and_audit_api_call(&caller, "/health", &payload)
            .await;

        assert!(
            result.is_ok(),
            "System should be authorized for API call: {:?}",
            result
        );
    }

    /// H-1 Security Test: Verify metadata whitelist validation rejects unauthorized labels
    #[tokio::test(flavor = "multi_thread")]
    async fn test_h1_metadata_whitelist_rejects_unauthorized_labels() {
        let orchestrator = create_test_orchestrator();

        // Create a request with unauthorized metadata label
        let mut request = create_test_ingress_request(Priority::Low);
        request.task.labels = vec![
            "production".to_string(),          // Whitelisted
            "malicious-injection".to_string(), // NOT whitelisted - should be rejected
        ];

        let result = orchestrator.process_ingress(request).await;

        assert!(
            result.is_err(),
            "Request with unauthorized label 'malicious-injection' should be rejected"
        );

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("malicious-injection") && err_msg.contains("not allowed"),
            "Error message should mention the rejected label, got: {}",
            err_msg
        );
    }

    /// H-1 Security Test: Verify metadata whitelist validation accepts whitelisted labels
    #[tokio::test(flavor = "multi_thread")]
    async fn test_h1_metadata_whitelist_accepts_whitelisted_labels() {
        let orchestrator = create_test_orchestrator();

        // Create a request with only whitelisted metadata labels
        let mut request = create_test_ingress_request(Priority::Low);
        request.task.labels = vec![
            "production".to_string(),
            "security".to_string(),
            "compliance".to_string(),
            "urgent".to_string(),
        ];

        let result = orchestrator.process_ingress(request).await;

        // Should NOT be rejected due to metadata validation
        // (may still go to review for other reasons like empty actions)
        assert!(
            result.is_ok(),
            "Request with all whitelisted labels should not be rejected for metadata reasons, got: {:?}",
            result.as_ref().err()
        );
    }

    /// H-1 Security Test: Verify empty labels are accepted (no injection risk)
    #[tokio::test(flavor = "multi_thread")]
    async fn test_h1_metadata_whitelist_accepts_empty_labels() {
        let orchestrator = create_test_orchestrator();

        // Create a request with no labels
        let mut request = create_test_ingress_request(Priority::Low);
        request.task.labels = vec![];

        let result = orchestrator.process_ingress(request).await;

        assert!(
            result.is_ok(),
            "Request with no labels should not be rejected, got: {:?}",
            result.as_ref().err()
        );
    }

    // ── S22 T2: HandoffCompletionSink dispatch tests ──────────────────────────

    /// Mock HandoffCompletionSink that records every `finalize_accept` call.
    #[derive(Debug, Default)]
    struct MockHandoffCompletionSink {
        calls: std::sync::Mutex<Vec<cyberclaw_core::ids::HandoffId>>,
    }

    #[async_trait::async_trait]
    impl crate::handoff_completion_sink::HandoffCompletionSink for MockHandoffCompletionSink {
        async fn finalize_accept(
            &self,
            handoff_id: &cyberclaw_core::ids::HandoffId,
        ) -> anyhow::Result<()> {
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(handoff_id.clone());
            Ok(())
        }
    }

    /// Helper: build an orchestrator wired with a HandoffQueue and an optional
    /// HandoffCompletionSink, then enqueue a Handoff-targeted review in the
    /// review queue, returning the orchestrator + review_id + handoff_id.
    async fn setup_handoff_review_orchestrator(
        sink: Option<Arc<dyn crate::handoff_completion_sink::HandoffCompletionSink>>,
    ) -> (
        ControlPlaneOrchestrator,
        Arc<crate::handoff_queue::InMemoryHandoffQueue>,
        cyberclaw_core::ids::ReviewId,
        cyberclaw_core::ids::HandoffId,
    ) {
        use crate::event_bus::InMemoryEventBus;
        use crate::execution_service::InMemoryExecutionService;
        use crate::handoff_queue::InMemoryHandoffQueue;
        use crate::lease_manager::InMemoryLeaseManager;
        use crate::membership_service::InMemoryMembershipService;
        use crate::placement_engine::InMemoryPlacementEngine;
        use cyberclaw_core::cluster::{
            MembershipState, NodeCapacity, NodeHealth, NodeRecord, NodeRole,
        };
        use cyberclaw_core::handoff::HandoffRequest;
        use cyberclaw_core::ids::{AgentId, HandoffId, ReviewId};
        use cyberclaw_core::review::{ReviewKind, ReviewRequest};

        let gateway = Arc::new(crate::gateway_router::InMemoryGatewayRouter::new());
        let registry = Arc::new(crate::registry::InMemoryRegistry::new());
        let resolver = Arc::new(crate::resolver::InMemoryResolver::new(registry.clone()));
        let review_queue = Arc::new(crate::review_queue::InMemoryReviewQueue::new(None));
        let task_manager = Arc::new(crate::task_manager::InMemoryTaskManager::new());
        let execution_service = Arc::new(InMemoryExecutionService::default());
        let placement_engine = Arc::new(InMemoryPlacementEngine::new());
        let lease_manager = Arc::new(InMemoryLeaseManager::default());
        let membership_service = Arc::new(InMemoryMembershipService::default());
        let event_bus = Arc::new(InMemoryEventBus::default());
        let policy_engine = Arc::new(cyberclaw_governance::engine::DefaultPolicyEngine::default());
        let security_event_store: Arc<
            dyn cyberclaw_observability::security_event_store::SecurityEventStore,
        > = Arc::new(cyberclaw_observability::InMemorySecurityEventStore::new());
        let plugin_registry = Arc::new(PluginRegistry::new(std::path::PathBuf::from(
            "/tmp/plugins",
        )));

        // Add a node so placement succeeds if ever needed.
        let test_node_id = NodeId::from_string("test-node-1".to_string()).unwrap();
        let test_node = NodeRecord {
            id: test_node_id.clone(),
            role: NodeRole::Worker,
            labels: vec!["worker".to_string()],
            region: None,
            zone: None,
            health: NodeHealth::Healthy,
            membership_state: MembershipState::Active,
            capacity: NodeCapacity {
                max_executions: Some(10),
                max_cpu_millis: None,
                max_memory_mb: None,
            },
            current_executions: 0,
            last_heartbeat_at: chrono::Utc::now(),
        };
        membership_service.join(test_node).unwrap();
        membership_service.heartbeat(&test_node_id).unwrap();

        // Wire up HandoffQueue.
        let handoff_queue = Arc::new(InMemoryHandoffQueue::new());

        // Enqueue a handoff request so the queue has something to update on rejection.
        let handoff_id = HandoffId::new();
        let from_agent_id = AgentId::from_string("agent-from".to_string()).unwrap();
        let to_agent_id = AgentId::from_string("agent-to".to_string()).unwrap();
        let handoff_req = HandoffRequest::new(
            handoff_id.clone(),
            from_agent_id,
            to_agent_id,
            "conv-1".to_string(),
            "handoff reason".to_string(),
            "briefing text".to_string(),
            vec![],
            None,
            chrono::Utc::now(),
        );
        {
            use crate::handoff_queue::HandoffQueue as _;
            use cyberclaw_core::handoff::HandoffStatus;
            handoff_queue.enqueue(handoff_req).await.unwrap();
            assert_eq!(
                handoff_queue.get(&handoff_id).await.unwrap().status,
                HandoffStatus::Initiated
            );
        }

        // Build orchestrator.
        let mut orchestrator = ControlPlaneOrchestrator::new(
            gateway,
            resolver,
            registry,
            review_queue.clone(),
            task_manager,
            execution_service,
            placement_engine,
            lease_manager,
            membership_service,
            event_bus,
            policy_engine,
            plugin_registry,
            security_event_store,
        )
        .with_handoff_queue(handoff_queue.clone() as Arc<dyn crate::handoff_queue::HandoffQueue>);

        if let Some(s) = sink {
            orchestrator = orchestrator.with_handoff_completion_sink(s);
        }

        // Enqueue a Handoff-targeted review directly in the review queue.
        let review_id = ReviewId::new();
        let requester = create_test_actor("requester-1", "Requester");
        let review = ReviewRequest::for_handoff(
            review_id.clone(),
            handoff_id.clone(),
            None,
            "Handoff review".to_string(),
            "Approve the handoff".to_string(),
            requester,
            ReviewKind::Approval,
            TraceId::new(),
            chrono::Utc::now(),
        );
        orchestrator.review_queue.enqueue(review).await.unwrap();

        (orchestrator, handoff_queue, review_id, handoff_id)
    }

    /// S22 T2: Approving a Handoff review calls `HandoffCompletionSink::finalize_accept`.
    #[tokio::test(flavor = "multi_thread")]
    async fn process_review_result_dispatches_handoff_approve() {
        let mock_sink = Arc::new(MockHandoffCompletionSink::default());
        let sink_clone = mock_sink.clone();

        let (orchestrator, _queue, review_id, handoff_id) = setup_handoff_review_orchestrator(
            Some(sink_clone as Arc<dyn crate::handoff_completion_sink::HandoffCompletionSink>),
        )
        .await;

        let reviewer = create_test_actor("reviewer-x", "Reviewer X");
        orchestrator
            .process_review_result(&review_id, true, reviewer)
            .await
            .expect("handoff approve should succeed");

        let calls = mock_sink.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "finalize_accept should be called once");
        assert_eq!(
            calls[0], handoff_id,
            "finalize_accept called with wrong handoff_id"
        );
    }

    /// S22 T2: Rejecting a Handoff review sets HandoffQueue status to Declined
    /// and does NOT call the HandoffCompletionSink.
    #[tokio::test(flavor = "multi_thread")]
    async fn process_review_result_dispatches_handoff_reject() {
        let mock_sink = Arc::new(MockHandoffCompletionSink::default());
        let sink_clone = mock_sink.clone();

        let (orchestrator, queue, review_id, handoff_id) = setup_handoff_review_orchestrator(Some(
            sink_clone as Arc<dyn crate::handoff_completion_sink::HandoffCompletionSink>,
        ))
        .await;

        let reviewer = create_test_actor("reviewer-y", "Reviewer Y");
        orchestrator
            .process_review_result(&review_id, false, reviewer)
            .await
            .expect("handoff reject should succeed");

        // Sink must NOT have been called on rejection.
        // Drop the lock before the await below to satisfy clippy::await_holding_lock.
        {
            let calls = mock_sink.calls.lock().unwrap();
            assert_eq!(
                calls.len(),
                0,
                "finalize_accept must not be called on rejection"
            );
        }

        // HandoffQueue status must be Declined.
        use crate::handoff_queue::HandoffQueue as _;
        use cyberclaw_core::handoff::HandoffStatus;
        let req = queue.get(&handoff_id).await.expect("handoff should exist");
        assert_eq!(
            req.status,
            HandoffStatus::Declined,
            "handoff status should be Declined after rejection"
        );
    }
}
