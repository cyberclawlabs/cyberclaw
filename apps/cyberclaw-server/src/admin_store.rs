//! In-memory admin-scope registry for the CyberClaw admin console.
//!
//! Sprint 8 Phase A. Backs the `/api/v1/{agents,skills,tasks,executions,
//! reviews,cluster/nodes}` admin surface with realistic in-process state
//! so the UI shows content immediately after `POST /admin/seed-demo`.
//!
//! This is **not** a first-class platform object (EVOLUTION_IDIOMS §5):
//! it's a concrete process-lifetime state holder, same taxonomy as
//! `channels_store` / `task_store`. No trait is exposed — if a future
//! sprint needs a trait, we refactor then.
//!
//! # Shape contract
//!
//! Field names mirror `MOCK.*` entries in `web/src/data.jsx` exactly;
//! the admin SPA uses the same keys to render every table.
//!
//! # Concurrency
//!
//! Each registry is a `tokio::sync::RwLock<Vec<T>>`. Handlers acquire a
//! read lock for listing and a write lock for mutations. The store holds
//! `Vec` (not `HashMap`) because (a) admin lists are short, (b) we want
//! insertion-order list semantics for the UI, and (c) id-lookups in `Vec`
//! are linear but the N is small.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::warn;

/// An agent row as seen by the admin SPA.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdminAgent {
    pub agent_id: String,
    pub name: String,
    pub description: String,
    /// `"high" | "medium" | "low"`
    pub trust_level: String,
    /// `"active" | "idle" | "paused"`
    pub status: String,
    pub tools: u32,
    pub skills: u32,
    pub executions_24h: u32,
}

/// A skill row as seen by the admin SPA.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdminSkill {
    pub skill_id: String,
    pub name: String,
    pub category: String,
    /// `"builtin" | "hub" | "local"`
    pub source: String,
    pub description: String,
    /// ISO-8601 date.
    pub installed_at: String,
    /// Whether the skill is enabled (default: true).
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// A task row (admin-scope projection — distinct from the `TaskWithStatus`
/// control-plane shape).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdminTask {
    pub task_id: String,
    pub description: String,
    /// `"pending" | "running" | "done" | "failed" | "cancelled"`
    pub status: String,
    /// ISO-8601 timestamp.
    pub created_at: String,
    /// `"MM:SS"` formatted elapsed time; `"—"` for pending tasks.
    pub elapsed: String,
    pub task_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// An execution row.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdminExecution {
    pub execution_id: String,
    pub agent: String,
    pub capability: String,
    /// `"running" | "done" | "failed" | "pending_review" | "cancelled"`
    pub status: String,
    /// `"low" | "medium" | "high" | "critical"`
    pub risk_level: String,
    pub started_at: String,
    pub duration: String,
}

/// A review row.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdminReview {
    pub review_id: String,
    pub execution_id: String,
    pub capability_id: String,
    pub actor: String,
    pub risk_level: String,
    pub reason_for_review: String,
    /// `"pending" | "approved" | "rejected"`
    pub status: String,
    pub proposed_input: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<String>,
}

/// A cluster node row.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdminNode {
    pub node_id: String,
    /// `"controller" | "worker"`
    pub role: String,
    /// `"leader" | "follower" | "online" | "draining" | "offline"`
    pub status: String,
    pub last_heartbeat: String,
    pub version: String,
    pub active_execs: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default)]
    pub cpu: u32,
    #[serde(default)]
    pub mem: u32,
}

/// Summary counts returned by `POST /admin/seed-demo`.
#[derive(Debug, Serialize)]
pub struct SeedCounts {
    pub agents: usize,
    pub skills: usize,
    pub tasks: usize,
    pub executions: usize,
    pub reviews: usize,
    pub nodes: usize,
}

/// Concrete admin-scope state holder (see module docs).
#[derive(Debug, Default)]
pub struct AdminStore {
    pub agents: RwLock<Vec<AdminAgent>>,
    pub skills: RwLock<Vec<AdminSkill>>,
    pub tasks: RwLock<Vec<AdminTask>>,
    pub executions: RwLock<Vec<AdminExecution>>,
    pub reviews: RwLock<Vec<AdminReview>>,
    pub nodes: RwLock<Vec<AdminNode>>,
    /// On-disk path for `agents.json`. `None` disables persistence.
    agents_path: Option<PathBuf>,
}

impl AdminStore {
    /// Construct a fresh, empty store (no persistence — used by unit tests).
    pub fn new() -> Self {
        Self::default()
    }

    /// Default agents path: `$HOME/.cyberclaw/agents.json`, overridable via
    /// `CYBERCLAW_AGENTS_PATH`. Falls back to `.cyberclaw/agents.json` in
    /// the current working dir when `$HOME` is unset.
    pub fn default_agents_path() -> PathBuf {
        if let Ok(custom) = std::env::var("CYBERCLAW_AGENTS_PATH") {
            return PathBuf::from(custom);
        }
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".cyberclaw").join("agents.json"))
            .unwrap_or_else(|| PathBuf::from(".cyberclaw").join("agents.json"))
    }

    /// Construct a store with disk-backed agents persistence. Reads any
    /// existing rows at `agents_path` into the `agents` registry.
    pub fn new_with_persistence(agents_path: PathBuf) -> Self {
        let agents = load_agents_from_disk(&agents_path);
        Self {
            agents: RwLock::new(agents),
            skills: RwLock::new(Vec::new()),
            tasks: RwLock::new(Vec::new()),
            executions: RwLock::new(Vec::new()),
            reviews: RwLock::new(Vec::new()),
            nodes: RwLock::new(Vec::new()),
            agents_path: Some(agents_path),
        }
    }

    /// Persist the current agents registry to `agents_path` (chmod 600 on
    /// unix). No-op when the store was constructed without a path. Logs
    /// (but does not propagate) I/O errors.
    pub async fn save_agents_to_disk(&self) {
        let Some(path) = self.agents_path.as_ref() else {
            return;
        };
        let snapshot = self.agents.read().await.clone();
        if let Err(e) = save_agents_to_path(path, &snapshot) {
            warn!(path = %path.display(), error = %e, "agents.json write failed");
        }
    }

    /// `true` when every registry is empty — used by
    /// `/admin/seed-demo` as a safeguard against double-seeding.
    pub async fn is_empty(&self) -> bool {
        self.agents.read().await.is_empty()
            && self.skills.read().await.is_empty()
            && self.tasks.read().await.is_empty()
            && self.executions.read().await.is_empty()
            && self.reviews.read().await.is_empty()
            && self.nodes.read().await.is_empty()
    }

    /// Populate every registry with realistic demo content. Returns the
    /// per-registry counts so the caller can verify the seed.
    pub async fn seed_demo(&self) -> SeedCounts {
        let agents = seed_agents();
        let skills = seed_skills();
        let tasks = seed_tasks();
        let executions = seed_executions();
        let reviews = seed_reviews();
        let nodes = seed_nodes();

        let counts = SeedCounts {
            agents: agents.len(),
            skills: skills.len(),
            tasks: tasks.len(),
            executions: executions.len(),
            reviews: reviews.len(),
            nodes: nodes.len(),
        };

        *self.agents.write().await = agents;
        *self.skills.write().await = skills;
        *self.tasks.write().await = tasks;
        *self.executions.write().await = executions;
        *self.reviews.write().await = reviews;
        *self.nodes.write().await = nodes;

        counts
    }
}

// ===== Seed data generators =====

fn now_minus(secs: i64) -> String {
    (Utc::now() - ChronoDuration::seconds(secs)).to_rfc3339()
}

fn iso_date(offset_days: i64) -> String {
    (Utc::now() - ChronoDuration::days(offset_days))
        .date_naive()
        .to_string()
}

fn seed_agents() -> Vec<AdminAgent> {
    vec![
        AdminAgent {
            agent_id: "ag_01H8X".into(),
            name: "research-analyst".into(),
            description: "Synthesizes web + doc research into structured briefs".into(),
            trust_level: "high".into(),
            status: "active".into(),
            tools: 14,
            skills: 6,
            executions_24h: 42,
        },
        AdminAgent {
            agent_id: "ag_01H9A".into(),
            name: "code-reviewer".into(),
            description: "Reviews PRs against house style; comments inline".into(),
            trust_level: "high".into(),
            status: "active".into(),
            tools: 9,
            skills: 4,
            executions_24h: 31,
        },
        AdminAgent {
            agent_id: "ag_01H9B".into(),
            name: "triage-bot".into(),
            description: "Classifies inbound tickets, routes to humans + sets SLA".into(),
            trust_level: "medium".into(),
            status: "active".into(),
            tools: 7,
            skills: 3,
            executions_24h: 128,
        },
        AdminAgent {
            agent_id: "ag_01HAC".into(),
            name: "release-captain".into(),
            description: "Orchestrates release checklist; requires approvals for production".into(),
            trust_level: "medium".into(),
            status: "idle".into(),
            tools: 22,
            skills: 8,
            executions_24h: 3,
        },
        AdminAgent {
            agent_id: "ag_01HBD".into(),
            name: "data-exfil-detector".into(),
            description: "Monitors connector calls for sensitive-data leakage".into(),
            trust_level: "high".into(),
            status: "active".into(),
            tools: 4,
            skills: 2,
            executions_24h: 912,
        },
        AdminAgent {
            agent_id: "ag_01HC3".into(),
            name: "onboarding-buddy".into(),
            description: "Guides new hires through week-1 tasks".into(),
            trust_level: "low".into(),
            status: "active".into(),
            tools: 6,
            skills: 5,
            executions_24h: 7,
        },
        AdminAgent {
            agent_id: "ag_01HD1".into(),
            name: "incident-commander".into(),
            description: "Declares + runs incident channels; pages on-call".into(),
            trust_level: "high".into(),
            status: "paused".into(),
            tools: 18,
            skills: 7,
            executions_24h: 0,
        },
        AdminAgent {
            agent_id: "ag_01HE7".into(),
            name: "finance-reconciler".into(),
            description: "Reconciles daily Stripe + ledger; flags anomalies".into(),
            trust_level: "medium".into(),
            status: "active".into(),
            tools: 5,
            skills: 3,
            executions_24h: 24,
        },
    ]
}

fn seed_skills() -> Vec<AdminSkill> {
    vec![
        AdminSkill {
            skill_id: "sk_pr_review".into(),
            name: "pull-request-review".into(),
            category: "Development".into(),
            source: "builtin".into(),
            description: "Walks a diff, posts contextual review comments".into(),
            installed_at: iso_date(65),
            enabled: true,
        },
        AdminSkill {
            skill_id: "sk_unit_test".into(),
            name: "unit-test-authoring".into(),
            category: "Development".into(),
            source: "builtin".into(),
            description: "Generates unit tests from a function signature + behaviors".into(),
            installed_at: iso_date(65),
            enabled: true,
        },
        AdminSkill {
            skill_id: "sk_migration".into(),
            name: "db-migration-writer".into(),
            category: "Development".into(),
            source: "hub".into(),
            description: "Drafts reversible DB migrations".into(),
            installed_at: iso_date(45),
            enabled: true,
        },
        AdminSkill {
            skill_id: "sk_web_search".into(),
            name: "web-research-synthesizer".into(),
            category: "Research".into(),
            source: "builtin".into(),
            description: "Plans + executes multi-step research, cites sources".into(),
            installed_at: iso_date(65),
            enabled: true,
        },
        AdminSkill {
            skill_id: "sk_lit_review".into(),
            name: "literature-review".into(),
            category: "Research".into(),
            source: "hub".into(),
            description: "Summarizes + clusters papers for systematic review".into(),
            installed_at: iso_date(30),
            enabled: true,
        },
        AdminSkill {
            skill_id: "sk_calendar".into(),
            name: "calendar-scheduler".into(),
            category: "Productivity".into(),
            source: "builtin".into(),
            description: "Finds meeting slots across calendars with tz awareness".into(),
            installed_at: iso_date(65),
            enabled: true,
        },
        AdminSkill {
            skill_id: "sk_inbox".into(),
            name: "inbox-triager".into(),
            category: "Productivity".into(),
            source: "builtin".into(),
            description: "Labels, snoozes, drafts replies for email triage".into(),
            installed_at: iso_date(65),
            enabled: true,
        },
        AdminSkill {
            skill_id: "sk_copy".into(),
            name: "copywriting-assistant".into(),
            category: "Creative".into(),
            source: "hub".into(),
            description: "Drafts on-brand copy from briefs".into(),
            installed_at: iso_date(27),
            enabled: true,
        },
        AdminSkill {
            skill_id: "sk_illu".into(),
            name: "illustration-director".into(),
            category: "Creative".into(),
            source: "local".into(),
            description: "Plans + orchestrates image gen pipelines".into(),
            installed_at: iso_date(17),
            enabled: true,
        },
        AdminSkill {
            skill_id: "sk_orch".into(),
            name: "agent-orchestrator".into(),
            category: "Agents".into(),
            source: "builtin".into(),
            description: "Spawns + coordinates sub-agents for parallel subtasks".into(),
            installed_at: iso_date(65),
            enabled: true,
        },
        AdminSkill {
            skill_id: "sk_mem".into(),
            name: "long-term-memory".into(),
            category: "Agents".into(),
            source: "hub".into(),
            description: "Persists episodic memory across sessions".into(),
            installed_at: iso_date(65),
            enabled: true,
        },
        AdminSkill {
            skill_id: "sk_misc".into(),
            name: "filesystem-toolkit".into(),
            category: "Other".into(),
            source: "local".into(),
            description: "Safe fs operations with path allow-listing".into(),
            installed_at: iso_date(8),
            enabled: true,
        },
    ]
}

fn seed_tasks() -> Vec<AdminTask> {
    vec![
        AdminTask {
            task_id: "t_01JX4A".into(),
            description: "Draft Q2 product brief from PRD + research notes".into(),
            status: "running".into(),
            created_at: now_minus(4 * 60),
            elapsed: "04:12".into(),
            task_type: "synthesis".into(),
            error: None,
        },
        AdminTask {
            task_id: "t_01JX4B".into(),
            description: "Triage 23 inbound security reports".into(),
            status: "running".into(),
            created_at: now_minus(11 * 60),
            elapsed: "11:38".into(),
            task_type: "triage".into(),
            error: None,
        },
        AdminTask {
            task_id: "t_01JX42".into(),
            description: "Reconcile Stripe payouts for yesterday".into(),
            status: "done".into(),
            created_at: now_minus(3600),
            elapsed: "18:44".into(),
            task_type: "reconcile".into(),
            error: None,
        },
        AdminTask {
            task_id: "t_01JX3Z".into(),
            description: "Generate release notes for v0.2.0".into(),
            status: "done".into(),
            created_at: now_minus(5400),
            elapsed: "02:11".into(),
            task_type: "synthesis".into(),
            error: None,
        },
        AdminTask {
            task_id: "t_01JX3X".into(),
            description: "Review PR #4429 — retry backoff refactor".into(),
            status: "done".into(),
            created_at: now_minus(7200),
            elapsed: "06:02".into(),
            task_type: "code_review".into(),
            error: None,
        },
        AdminTask {
            task_id: "t_01JX3U".into(),
            description: "Schedule all-hands across 4 timezones".into(),
            status: "done".into(),
            created_at: now_minus(8400),
            elapsed: "01:03".into(),
            task_type: "scheduling".into(),
            error: None,
        },
        AdminTask {
            task_id: "t_01JX3S".into(),
            description: "Classify support ticket #19203".into(),
            status: "failed".into(),
            created_at: now_minus(9100),
            elapsed: "00:08".into(),
            task_type: "triage".into(),
            error: Some("connector \"zendesk.read_ticket\" returned 401".into()),
        },
        AdminTask {
            task_id: "t_01JX3Q".into(),
            description: "Produce weekly engineering digest".into(),
            status: "pending".into(),
            created_at: now_minus(200),
            elapsed: "—".into(),
            task_type: "synthesis".into(),
            error: None,
        },
        AdminTask {
            task_id: "t_01JX3P".into(),
            description: "Backfill Notion tags on archived docs".into(),
            status: "pending".into(),
            created_at: now_minus(90),
            elapsed: "—".into(),
            task_type: "backfill".into(),
            error: None,
        },
        AdminTask {
            task_id: "t_01JX3M".into(),
            description: "Kick off incident IR-488".into(),
            status: "cancelled".into(),
            created_at: now_minus(13400),
            elapsed: "00:22".into(),
            task_type: "incident".into(),
            error: None,
        },
        AdminTask {
            task_id: "t_01JX3L".into(),
            description: "Deprecate legacy /v0 chat endpoint".into(),
            status: "cancelled".into(),
            created_at: now_minus(20400),
            elapsed: "00:04".into(),
            task_type: "cleanup".into(),
            error: None,
        },
        AdminTask {
            task_id: "t_01JX3K".into(),
            description: "Summarize support conversations from past 7 days".into(),
            status: "done".into(),
            created_at: now_minus(24000),
            elapsed: "12:17".into(),
            task_type: "synthesis".into(),
            error: None,
        },
        AdminTask {
            task_id: "t_01JX3J".into(),
            description: "Reconcile ledger for EU entity".into(),
            status: "done".into(),
            created_at: now_minus(28000),
            elapsed: "31:49".into(),
            task_type: "reconcile".into(),
            error: None,
        },
        AdminTask {
            task_id: "t_01JX3H".into(),
            description: "Post weekly digest to #eng-all".into(),
            status: "done".into(),
            created_at: now_minus(31000),
            elapsed: "00:41".into(),
            task_type: "publish".into(),
            error: None,
        },
        AdminTask {
            task_id: "t_01JX3G".into(),
            description: "Propagate GDPR deletion request dsr-2041".into(),
            status: "failed".into(),
            created_at: now_minus(33000),
            elapsed: "00:12".into(),
            task_type: "compliance".into(),
            error: Some("downstream connector \"vault.redact\" timed out after 30s".into()),
        },
    ]
}

fn seed_executions() -> Vec<AdminExecution> {
    vec![
        AdminExecution {
            execution_id: "ex_01N9F2".into(),
            agent: "research-analyst".into(),
            capability: "web.search".into(),
            status: "running".into(),
            risk_level: "low".into(),
            started_at: now_minus(120),
            duration: "02:00".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9F1".into(),
            agent: "code-reviewer".into(),
            capability: "github.read_pr".into(),
            status: "running".into(),
            risk_level: "medium".into(),
            started_at: now_minus(40),
            duration: "00:40".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9F0".into(),
            agent: "release-captain".into(),
            capability: "k8s.rollout_deployment".into(),
            status: "pending_review".into(),
            risk_level: "critical".into(),
            started_at: now_minus(75),
            duration: "01:15".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EZ".into(),
            agent: "finance-reconciler".into(),
            capability: "stripe.list_payouts".into(),
            status: "pending_review".into(),
            risk_level: "high".into(),
            started_at: now_minus(300),
            duration: "00:12".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EY".into(),
            agent: "triage-bot".into(),
            capability: "notion.read_page".into(),
            status: "done".into(),
            risk_level: "low".into(),
            started_at: now_minus(980),
            duration: "00:04".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EX".into(),
            agent: "data-exfil-detector".into(),
            capability: "fs.read".into(),
            status: "done".into(),
            risk_level: "low".into(),
            started_at: now_minus(1120),
            duration: "00:02".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EW".into(),
            agent: "code-reviewer".into(),
            capability: "github.read_pr".into(),
            status: "done".into(),
            risk_level: "low".into(),
            started_at: now_minus(1340),
            duration: "00:08".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EV".into(),
            agent: "release-captain".into(),
            capability: "cmd.exec".into(),
            status: "failed".into(),
            risk_level: "high".into(),
            started_at: now_minus(2400),
            duration: "00:31".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EU".into(),
            agent: "onboarding-buddy".into(),
            capability: "notion.read_page".into(),
            status: "done".into(),
            risk_level: "medium".into(),
            started_at: now_minus(3100),
            duration: "00:06".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9ET".into(),
            agent: "finance-reconciler".into(),
            capability: "stripe.list_charges".into(),
            status: "done".into(),
            risk_level: "low".into(),
            started_at: now_minus(3800),
            duration: "00:19".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9ES".into(),
            agent: "incident-commander".into(),
            capability: "pagerduty.trigger_incident".into(),
            status: "cancelled".into(),
            risk_level: "high".into(),
            started_at: now_minus(5200),
            duration: "00:02".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9ER".into(),
            agent: "research-analyst".into(),
            capability: "notion.create_page".into(),
            status: "done".into(),
            risk_level: "medium".into(),
            started_at: now_minus(6400),
            duration: "00:44".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EQ".into(),
            agent: "research-analyst".into(),
            capability: "web.search".into(),
            status: "done".into(),
            risk_level: "low".into(),
            started_at: now_minus(7200),
            duration: "00:38".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EP".into(),
            agent: "code-reviewer".into(),
            capability: "github.read_pr".into(),
            status: "done".into(),
            risk_level: "medium".into(),
            started_at: now_minus(8400),
            duration: "00:22".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EO".into(),
            agent: "release-captain".into(),
            capability: "cmd.exec".into(),
            status: "done".into(),
            risk_level: "high".into(),
            started_at: now_minus(9600),
            duration: "00:58".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EN".into(),
            agent: "triage-bot".into(),
            capability: "notion.read_page".into(),
            status: "done".into(),
            risk_level: "low".into(),
            started_at: now_minus(11200),
            duration: "00:06".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EM".into(),
            agent: "finance-reconciler".into(),
            capability: "stripe.list_payouts".into(),
            status: "done".into(),
            risk_level: "medium".into(),
            started_at: now_minus(12400),
            duration: "00:33".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EL".into(),
            agent: "data-exfil-detector".into(),
            capability: "fs.read".into(),
            status: "done".into(),
            risk_level: "low".into(),
            started_at: now_minus(13600),
            duration: "00:05".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EK".into(),
            agent: "onboarding-buddy".into(),
            capability: "notion.create_page".into(),
            status: "done".into(),
            risk_level: "medium".into(),
            started_at: now_minus(14800),
            duration: "00:27".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EJ".into(),
            agent: "code-reviewer".into(),
            capability: "cmd.exec".into(),
            status: "failed".into(),
            risk_level: "high".into(),
            started_at: now_minus(16000),
            duration: "00:14".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EI".into(),
            agent: "research-analyst".into(),
            capability: "web.search".into(),
            status: "done".into(),
            risk_level: "low".into(),
            started_at: now_minus(17200),
            duration: "00:29".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EH".into(),
            agent: "incident-commander".into(),
            capability: "cmd.exec".into(),
            status: "done".into(),
            risk_level: "critical".into(),
            started_at: now_minus(18400),
            duration: "01:47".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EG".into(),
            agent: "finance-reconciler".into(),
            capability: "stripe.list_charges".into(),
            status: "done".into(),
            risk_level: "medium".into(),
            started_at: now_minus(19600),
            duration: "00:16".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EF".into(),
            agent: "release-captain".into(),
            capability: "cmd.exec".into(),
            status: "failed".into(),
            risk_level: "critical".into(),
            started_at: now_minus(20800),
            duration: "00:08".into(),
        },
        AdminExecution {
            execution_id: "ex_01N9EE".into(),
            agent: "triage-bot".into(),
            capability: "notion.read_page".into(),
            status: "done".into(),
            risk_level: "low".into(),
            started_at: now_minus(22000),
            duration: "00:03".into(),
        },
    ]
}

fn seed_reviews() -> Vec<AdminReview> {
    vec![
        AdminReview {
            review_id: "rv_01Q7A1".into(),
            execution_id: "ex_01N9F0".into(),
            capability_id: "cmd.exec".into(),
            actor: "release-captain".into(),
            risk_level: "critical".into(),
            reason_for_review: "DangerousCapabilityFilter rule #12: shell command targets prod namespace; requires human approval.".into(),
            status: "pending".into(),
            proposed_input: serde_json::json!({
                "command": "kubectl rollout restart deploy/payments-api -n payments",
                "cluster": "prod-us-east",
                "timeout_secs": 180,
            }),
            reason: None,
            decided_by: None,
            decided_at: None,
        },
        AdminReview {
            review_id: "rv_01Q7A0".into(),
            execution_id: "ex_01N9EZ".into(),
            capability_id: "fs.delete".into(),
            actor: "data-exfil-detector".into(),
            risk_level: "high".into(),
            reason_for_review: "fs.delete operation targets path outside workspace sandbox (/var/log/archive/*).".into(),
            status: "pending".into(),
            proposed_input: serde_json::json!({
                "path": "/var/log/archive/audit-2026-03.jsonl.gz",
                "recursive": false,
            }),
            reason: None,
            decided_by: None,
            decided_at: None,
        },
        AdminReview {
            review_id: "rv_01Q78F".into(),
            execution_id: "ex_01N9EK".into(),
            capability_id: "notion.create_page".into(),
            actor: "onboarding-buddy".into(),
            risk_level: "high".into(),
            reason_for_review: "Page shared with external workspace.".into(),
            status: "approved".into(),
            proposed_input: serde_json::json!({
                "parent": "workspace:cyberclaw",
                "title": "Onboarding Week 1",
            }),
            reason: None,
            decided_by: Some("op_ada".into()),
            decided_at: Some(now_minus(2 * 3600)),
        },
        AdminReview {
            review_id: "rv_01Q77Y".into(),
            execution_id: "ex_01N9ER".into(),
            capability_id: "notion.create_page".into(),
            actor: "research-analyst".into(),
            risk_level: "medium".into(),
            reason_for_review: "Publication to public workspace.".into(),
            status: "approved".into(),
            proposed_input: serde_json::json!({
                "parent": "workspace:cyberclaw",
                "title": "Q1 competitive landscape",
            }),
            reason: None,
            decided_by: Some("op_liu".into()),
            decided_at: Some(now_minus(9 * 3600)),
        },
        AdminReview {
            review_id: "rv_01Q784".into(),
            execution_id: "ex_01N9EF".into(),
            capability_id: "cmd.exec".into(),
            actor: "release-captain".into(),
            risk_level: "critical".into(),
            reason_for_review: "Shell command grants * on * — overly broad scope.".into(),
            status: "rejected".into(),
            proposed_input: serde_json::json!({
                "command": "aws iam attach-role-policy --role-name * --policy-arn *",
            }),
            reason: Some("Grants * on * — too broad. Scope to payments role.".into()),
            decided_by: Some("op_ada".into()),
            decided_at: Some(now_minus(5 * 3600)),
        },
    ]
}

fn seed_nodes() -> Vec<AdminNode> {
    vec![
        AdminNode {
            node_id: "node-01".into(),
            role: "controller".into(),
            status: "leader".into(),
            last_heartbeat: now_minus(1),
            version: "0.1.0".into(),
            active_execs: 3,
            region: Some("us-east-1".into()),
            cpu: 18,
            mem: 42,
        },
        AdminNode {
            node_id: "node-02".into(),
            role: "controller".into(),
            status: "follower".into(),
            last_heartbeat: now_minus(2),
            version: "0.1.0".into(),
            active_execs: 0,
            region: Some("us-west-2".into()),
            cpu: 9,
            mem: 36,
        },
        AdminNode {
            node_id: "node-03".into(),
            role: "worker".into(),
            status: "follower".into(),
            last_heartbeat: now_minus(3),
            version: "0.0.9".into(),
            active_execs: 2,
            region: Some("eu-west-1".into()),
            cpu: 22,
            mem: 44,
        },
    ]
}

/// Helper used by the reviews API to project a pending review's proposed
/// input into the "as-if-dispatched" shape the UI renders inline.
pub fn summarize_proposed_input(value: &serde_json::Value) -> String {
    if let Some(obj) = value.as_object() {
        obj.iter()
            .map(|(k, v)| format!("{}={}", k, flatten_scalar(v)))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        value.to_string()
    }
}

// ---------------------------------------------------------------------------
// Disk I/O helpers for AdminAgent persistence
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
struct AgentsFile {
    #[serde(default)]
    agents: Vec<AdminAgent>,
}

/// Load agents from `path`. Returns an empty vec if the file is missing or
/// unparseable (logged as a warning).
pub fn load_agents_from_disk(path: &Path) -> Vec<AdminAgent> {
    if !path.exists() {
        return Vec::new();
    }
    match std::fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str::<AgentsFile>(&s) {
            Ok(file) => file.agents,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "agents.json parse failed; starting empty");
                Vec::new()
            }
        },
        Err(e) => {
            warn!(path = %path.display(), error = %e, "agents.json read failed; starting empty");
            Vec::new()
        }
    }
}

fn save_agents_to_path(path: &Path, agents: &[AdminAgent]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create_dir_all: {e}"))?;
    }
    let file = AgentsFile {
        agents: agents.to_vec(),
    };
    let json = serde_json::to_string_pretty(&file).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(path, &json).map_err(|e| format!("write: {e}"))?;
    // chmod 600 — only owner can read/write
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn flatten_scalar(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".into(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => format!("[{} items]", arr.len()),
        serde_json::Value::Object(obj) => format!("{{{} fields}}", obj.len()),
    }
}

/// Build the synthetic trace tree for `GET /api/v1/executions/:id/trace`.
///
/// Sprint 8 Phase A — returns a deterministic span tree keyed off the
/// execution id so repeat calls render stably in the UI. Real trace data
/// will flow from `EventRecorder` in a later phase.
pub fn synthetic_trace(execution_id: &str) -> serde_json::Value {
    serde_json::json!({
        "execution_id": execution_id,
        "total_ms": 1820,
        "spans": [
            {
                "id": "s1",
                "name": "agent.invoke",
                "kind": "agent",
                "started_at": now_minus(1820 / 1000),
                "duration_ms": 1820,
                "status": "ok",
                "attrs": { "agent": "release-captain" },
                "children": [
                    {
                        "id": "s1-1",
                        "name": "llm.plan",
                        "kind": "llm",
                        "started_at": now_minus(1810 / 1000),
                        "duration_ms": 412,
                        "status": "ok",
                        "children": []
                    },
                    {
                        "id": "s1-2",
                        "name": "skill.release_checklist",
                        "kind": "skill",
                        "started_at": now_minus(1400 / 1000),
                        "duration_ms": 186,
                        "status": "ok",
                        "children": [
                            {
                                "id": "s1-2-1",
                                "name": "fs.read",
                                "kind": "tool",
                                "started_at": now_minus(1390 / 1000),
                                "duration_ms": 18,
                                "status": "ok",
                                "children": []
                            }
                        ]
                    },
                    {
                        "id": "s1-3",
                        "name": "capability.cmd.exec",
                        "kind": "capability",
                        "started_at": now_minus(1200 / 1000),
                        "duration_ms": 310,
                        "status": "ok",
                        "children": []
                    }
                ]
            }
        ]
    })
}

/// Returns the 12-entry sample capability grouping used as the response
/// envelope for `GET /api/v1/capabilities/discover` when no real
/// connectors have registered capabilities yet. The `connector_count` is
/// sourced from the live `ConnectorRegistry` at call-site so the UI shows
/// the real registration count even on an empty platform.
pub fn sample_capabilities() -> HashMap<String, Vec<serde_json::Value>> {
    let mut map: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    map.insert(
        "local".to_string(),
        vec![
            serde_json::json!({
                "id": "fs.read",
                "name": "Read file",
                "risk_level": "low",
                "effects": ["Read"],
                "description": "Read a file from the workspace",
            }),
            serde_json::json!({
                "id": "fs.write",
                "name": "Write file",
                "risk_level": "medium",
                "effects": ["Write"],
                "description": "Create or overwrite a file in the workspace",
            }),
            serde_json::json!({
                "id": "cmd.exec",
                "name": "Execute shell command",
                "risk_level": "high",
                "effects": ["Write", "Execute"],
                "description": "Execute a shell command in the sandbox",
            }),
        ],
    );
    map.insert(
        "web".to_string(),
        vec![
            serde_json::json!({
                "id": "web.fetch",
                "name": "Fetch URL",
                "risk_level": "low",
                "effects": ["Read"],
                "description": "Fetch a URL and return the body",
            }),
            serde_json::json!({
                "id": "web.search",
                "name": "Search the web",
                "risk_level": "low",
                "effects": ["Read"],
                "description": "Run a web search",
            }),
        ],
    );
    map.insert(
        "github".to_string(),
        vec![
            serde_json::json!({
                "id": "github.read_pr",
                "name": "Read PR",
                "risk_level": "low",
                "effects": ["Read"],
                "description": "Fetch PR metadata, files, reviews",
            }),
            serde_json::json!({
                "id": "github.post_review_comment",
                "name": "Post review comment",
                "risk_level": "medium",
                "effects": ["Write"],
                "description": "Post an inline review comment on a PR",
            }),
            serde_json::json!({
                "id": "github.merge_pr",
                "name": "Merge PR",
                "risk_level": "high",
                "effects": ["Write", "Execute"],
                "description": "Merge a pull request",
            }),
        ],
    );
    map.insert(
        "notion".to_string(),
        vec![
            serde_json::json!({
                "id": "notion.read_page",
                "name": "Read page",
                "risk_level": "low",
                "effects": ["Read"],
                "description": "Read a Notion page",
            }),
            serde_json::json!({
                "id": "notion.create_page",
                "name": "Create page",
                "risk_level": "medium",
                "effects": ["Write"],
                "description": "Create a Notion page",
            }),
        ],
    );
    map.insert(
        "memory".to_string(),
        vec![
            serde_json::json!({
                "id": "memory.read",
                "name": "Read memory",
                "risk_level": "low",
                "effects": ["Read"],
                "description": "Read from semantic memory",
            }),
            serde_json::json!({
                "id": "memory.write",
                "name": "Write memory",
                "risk_level": "medium",
                "effects": ["Write"],
                "description": "Write a record to semantic memory",
            }),
        ],
    );
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn admin_store_new_is_empty() {
        let store = AdminStore::new();
        assert!(store.is_empty().await);
        assert!(store.agents.read().await.is_empty());
        assert!(store.skills.read().await.is_empty());
        assert!(store.tasks.read().await.is_empty());
        assert!(store.executions.read().await.is_empty());
        assert!(store.reviews.read().await.is_empty());
        assert!(store.nodes.read().await.is_empty());
    }

    #[tokio::test]
    async fn admin_store_seed_populates_all_registries() {
        let store = AdminStore::new();
        let counts = store.seed_demo().await;
        assert_eq!(counts.agents, 8);
        assert_eq!(counts.skills, 12);
        assert_eq!(counts.tasks, 15);
        assert_eq!(counts.executions, 25);
        assert_eq!(counts.reviews, 5);
        assert_eq!(counts.nodes, 3);

        // And the lock values match the returned counts.
        assert_eq!(store.agents.read().await.len(), 8);
        assert_eq!(store.skills.read().await.len(), 12);
        assert_eq!(store.tasks.read().await.len(), 15);
        assert_eq!(store.executions.read().await.len(), 25);
        assert_eq!(store.reviews.read().await.len(), 5);
        assert_eq!(store.nodes.read().await.len(), 3);
        assert!(!store.is_empty().await);
    }

    #[tokio::test]
    async fn seed_review_statuses_include_pending_approved_rejected() {
        let store = AdminStore::new();
        store.seed_demo().await;
        let reviews = store.reviews.read().await;
        let pending = reviews.iter().filter(|r| r.status == "pending").count();
        let approved = reviews.iter().filter(|r| r.status == "approved").count();
        let rejected = reviews.iter().filter(|r| r.status == "rejected").count();
        assert_eq!(pending, 2);
        assert_eq!(approved, 2);
        assert_eq!(rejected, 1);
    }

    #[tokio::test]
    async fn seed_task_statuses_spread_matches_spec() {
        let store = AdminStore::new();
        store.seed_demo().await;
        let tasks = store.tasks.read().await;
        let pending = tasks.iter().filter(|t| t.status == "pending").count();
        let running = tasks.iter().filter(|t| t.status == "running").count();
        let done = tasks.iter().filter(|t| t.status == "done").count();
        let failed = tasks.iter().filter(|t| t.status == "failed").count();
        let cancelled = tasks.iter().filter(|t| t.status == "cancelled").count();
        assert_eq!(pending, 2);
        assert_eq!(running, 2);
        assert_eq!(done, 7);
        assert_eq!(failed, 2);
        assert_eq!(cancelled, 2);
    }

    #[test]
    fn summarize_proposed_input_flattens_object() {
        let v = serde_json::json!({ "a": 1, "b": "x" });
        let s = summarize_proposed_input(&v);
        assert!(s.contains("a=1"));
        assert!(s.contains("b=x"));
    }

    #[test]
    fn synthetic_trace_has_execution_id_and_spans() {
        let t = synthetic_trace("ex_01N9F0");
        assert_eq!(t["execution_id"], "ex_01N9F0");
        assert!(t["spans"].is_array());
    }

    #[test]
    fn sample_capabilities_has_multiple_connectors_and_entries() {
        let m = sample_capabilities();
        // At least 5 sample connectors.
        assert!(m.len() >= 5);
        // Total capability count >= 12 (spec minimum).
        let total: usize = m.values().map(|v| v.len()).sum();
        assert!(total >= 12, "total capabilities {} < 12", total);
    }
}
