//! Lightweight keyword-based intent classifier.
//!
//! Route user requests to the right downstream handler **without** an LLM
//! round-trip. The classifier is intentionally narrow: it recognizes a fixed
//! set of platform-level intents (create skill, create agent, brainstorm,
//! daily digest) so the control plane can forward the payload to the right
//! facade without forcing the caller to know the internal routing table.
//!
//! # Architectural placement
//!
//! - **Not** a replacement for LLM-based NLU; this is a fast pre-filter.
//! - **Not** a first-class platform object (per `AGENTS.md §2.3` — no sixth
//!   object).
//! - Lives in `control-plane` because its sole job is to help the router
//!   decide which downstream facade (e.g. [`crate::skill_creator::SkillCreator`])
//!   should handle the payload.
//! - Uses a per-intent keyword list in Simplified Chinese + English, so
//!   multilingual prompts are covered without i18n injection.
//!
//! # Scope limits (Sprint 8)
//!
//! - No regex, no embeddings, no LLM — just case-insensitive substring
//!   matching with a deterministic priority order.
//! - No chat-handler integration — the endpoints that want classification
//!   call [`IntentClassifier::classify`] directly.
//! - Returns `None` when no intent matches; callers must fall back to their
//!   existing default path.

use std::collections::HashMap;

/// Platform-level intent categories recognised by the classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Intent {
    /// "帮我做一个 skill" / "create a skill" / "write a new skill".
    CreateSkill,
    /// "新建一个 agent" / "create an agent".
    CreateAgent,
    /// "头脑风暴 / brainstorm the design for ...".
    Brainstorm,
    /// "每日总结 / daily digest".
    DailyDigest,
    /// Sprint 12 L1 — conversational approval reply.
    /// "同意 / 批准 / approve / go / 确认".
    ApproveRequest,
    /// Sprint 12 L1 — conversational rejection reply.
    /// "拒绝 / 不同意 / reject / stop / deny".
    RejectRequest,
    /// OMC Hybrid-C — "帮我规划 / plan this / break down / 拆解 / make a plan".
    /// Routes to the planner facade for task decomposition and sprint layout.
    PlanRequest,
    /// OMC Hybrid-C — "并行 / multi-agent / orchestrate / 协调多个 agent / dispatch parallel".
    /// Routes to the orchestrator facade for parallel sub-agent dispatch.
    OrchestrateRequest,
    /// OMC Hybrid-C — "做成 skill / save as skill / 这个流程沉淀 / create skill from this".
    /// Routes to the skill_creator facade to distill the current conversation
    /// into a reusable skill. More specific than [`Intent::CreateSkill`] because
    /// it operates on *existing* context, not a free-form authoring request.
    SkillifyRequest,
    /// OMC Hybrid-C — "deep-analyze / 深度分析 / ultrathink".
    /// Routes to the deep_analyzer facade for heavy-context reasoning.
    /// Aligns with OMC's `ultrathink` / `deep-analyze` keyword triggers.
    DeepAnalyze,
}

impl Intent {
    /// Downstream facade hint for router wiring.
    ///
    /// Returns a stable `&'static str` identifier that the chat handler (and
    /// any future generic dispatcher) can match on without needing to know the
    /// full `Intent` enum. Aligns with OMC Hybrid-C: explicit keyword triggers
    /// map to well-known facade names.
    pub fn target_facade(&self) -> &'static str {
        match self {
            Intent::CreateSkill => "skill_creator",
            Intent::CreateAgent => "agent_creator",
            Intent::Brainstorm => "brainstormer",
            Intent::DailyDigest => "daily_digest",
            Intent::ApproveRequest => "approval_gateway",
            Intent::RejectRequest => "approval_gateway",
            Intent::PlanRequest => "planner",
            Intent::OrchestrateRequest => "orchestrator",
            Intent::SkillifyRequest => "skill_creator",
            Intent::DeepAnalyze => "deep_analyzer",
        }
    }
}

/// Classification result with the matched keyword, so callers can surface
/// provenance (useful for UI hints and audit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentMatch {
    pub intent: Intent,
    pub keyword: String,
}

/// Narrow keyword classifier. Construct once per server process; `classify`
/// is a pure function.
#[derive(Debug)]
pub struct IntentClassifier {
    /// Per-intent keyword list, already lowercased at build time.
    keywords: HashMap<Intent, Vec<String>>,
    /// Deterministic priority order for ties. First match wins.
    priority: Vec<Intent>,
}

impl IntentClassifier {
    /// Build with the default platform-level keyword set.
    ///
    /// Priority (tie break, first match wins):
    /// SkillifyRequest > CreateSkill > CreateAgent > PlanRequest >
    /// OrchestrateRequest > DeepAnalyze > Brainstorm > DailyDigest >
    /// ApproveRequest > RejectRequest.
    ///
    /// Rationale:
    /// - `SkillifyRequest` beats `CreateSkill` because "save as skill" / "这个
    ///   流程沉淀" semantically specialises the generic "create skill" intent
    ///   and should not be shadowed by it.
    /// - `DeepAnalyze` beats `Brainstorm` because OMC's `ultrathink` /
    ///   `deep-analyze` keywords request heavy reasoning, whereas brainstorm
    ///   is advisory-only.
    /// - Approve/Reject remain last so "approve creating a new skill called …"
    ///   still routes to the correct create-* facade.
    pub fn with_defaults() -> Self {
        let mut keywords: HashMap<Intent, Vec<String>> = HashMap::new();
        keywords.insert(
            Intent::CreateSkill,
            vec![
                "create a skill",
                "create skill",
                "new skill",
                "write a skill",
                "write new skill",
                "make a skill",
                "build a skill",
                "帮我做一个 skill",
                "做一个 skill",
                "创建 skill",
                "新建 skill",
                "写一个 skill",
                "做一个技能",
                "帮我做一个技能",
                "创建技能",
                "新建技能",
            ]
            .into_iter()
            .map(|s| s.to_lowercase())
            .collect(),
        );
        keywords.insert(
            Intent::CreateAgent,
            vec![
                "create an agent",
                "create agent",
                "new agent",
                "make an agent",
                "新建 agent",
                "创建 agent",
                "做一个 agent",
                "新建智能体",
                "创建智能体",
                "做一个角色",
            ]
            .into_iter()
            .map(|s| s.to_lowercase())
            .collect(),
        );
        keywords.insert(
            Intent::Brainstorm,
            vec![
                "brainstorm",
                "头脑风暴",
                "打磨想法",
                "think through",
                "help me design",
                "帮我设计",
            ]
            .into_iter()
            .map(|s| s.to_lowercase())
            .collect(),
        );
        keywords.insert(
            Intent::DailyDigest,
            vec![
                "daily digest",
                "daily summary",
                "每日总结",
                "每日简报",
                "today's digest",
                "今天做了什么",
            ]
            .into_iter()
            .map(|s| s.to_lowercase())
            .collect(),
        );
        // Sprint 12 L1 — conversational approval keywords.
        //
        // Kept narrow so benign chat ("this looks good, go ahead and explain")
        // doesn't trip the gate; the chat approval endpoint expects an explicit
        // request_id anyway, so these keywords are provenance metadata only.
        keywords.insert(
            Intent::ApproveRequest,
            vec![
                "approve",
                "approved",
                "go ahead",
                "looks good, go",
                "lgtm",
                "同意",
                "批准",
                "确认",
                "通过",
                "可以执行",
                "同意执行",
            ]
            .into_iter()
            .map(|s| s.to_lowercase())
            .collect(),
        );
        keywords.insert(
            Intent::RejectRequest,
            vec![
                "reject",
                "rejected",
                "deny",
                "denied",
                "stop",
                "abort",
                "拒绝",
                "不同意",
                "不批准",
                "驳回",
                "停止",
                "别执行",
            ]
            .into_iter()
            .map(|s| s.to_lowercase())
            .collect(),
        );

        // OMC Hybrid-C — planning facade trigger.
        //
        // Narrow to phrases that clearly ask for decomposition / sprint layout;
        // plain "plan" is intentionally excluded to avoid matching "plan on",
        // "floor plan", etc.
        keywords.insert(
            Intent::PlanRequest,
            vec![
                "plan this",
                "make a plan",
                "make me a plan",
                "break this down",
                "break it down",
                "break down the",
                "lay out a plan",
                "draft a plan",
                "plan the work",
                "plan out",
                "帮我规划",
                "帮我拆解",
                "帮我做个计划",
                "做一个规划",
                "做个规划",
                "拆解任务",
                "拆解一下",
                "制定计划",
                "规划一下",
            ]
            .into_iter()
            .map(|s| s.to_lowercase())
            .collect(),
        );

        // OMC Hybrid-C — orchestration facade trigger.
        //
        // Routes to the orchestrator when the user explicitly asks for
        // multi-agent / parallel dispatch. Generic "run in parallel" keywords
        // are included, but kept narrow enough that shell-level "run in
        // parallel with &" doesn't misroute a chat prompt.
        keywords.insert(
            Intent::OrchestrateRequest,
            vec![
                "orchestrate",
                "multi-agent",
                "multi agent",
                "dispatch parallel",
                "dispatch in parallel",
                "parallel agents",
                "run in parallel",
                "fan out to agents",
                "coordinate agents",
                "协调多个 agent",
                "协调多个智能体",
                "并行执行",
                "并行调度",
                "并行派发",
                "多 agent 协作",
                "多智能体协作",
                "派发并行任务",
            ]
            .into_iter()
            .map(|s| s.to_lowercase())
            .collect(),
        );

        // OMC Hybrid-C — skillify facade trigger.
        //
        // Distinct from `CreateSkill`: here the user wants to *distill the
        // current workflow* into a reusable skill package. Placed ahead of
        // `CreateSkill` in priority so "save this as a skill" is never
        // mis-routed to the generic skill authoring path.
        keywords.insert(
            Intent::SkillifyRequest,
            vec![
                "save as skill",
                "save as a skill",
                "save this as a skill",
                "save it as a skill",
                "create skill from this",
                "create a skill from this",
                "make this a skill",
                "turn this into a skill",
                "skillify",
                "做成 skill",
                "做成一个 skill",
                "沉淀成 skill",
                "沉淀为 skill",
                "这个流程沉淀",
                "把这个流程沉淀",
                "把这个做成 skill",
                "把这流程做成 skill",
                "固化成 skill",
            ]
            .into_iter()
            .map(|s| s.to_lowercase())
            .collect(),
        );

        // OMC Hybrid-C — deep analysis facade trigger.
        //
        // Matches OMC's `ultrathink` / `deep-analyze` keyword family so chat
        // users get the same escalation surface as CLI users.
        keywords.insert(
            Intent::DeepAnalyze,
            vec![
                "deep-analyze",
                "deep analyze",
                "deep analysis",
                "ultrathink",
                "ultra think",
                "think deeply",
                "深度分析",
                "深度思考",
                "深入分析",
                "深入思考",
                "彻底分析",
                "好好想想",
            ]
            .into_iter()
            .map(|s| s.to_lowercase())
            .collect(),
        );

        let priority = vec![
            // OMC Hybrid-C — SkillifyRequest must outrank CreateSkill: "save
            // this as a skill" is strictly more specific than "create a skill".
            Intent::SkillifyRequest,
            Intent::CreateSkill,
            Intent::CreateAgent,
            // Planning / orchestration land above Brainstorm because they
            // carry concrete routing side-effects, whereas Brainstorm is
            // advisory-only.
            Intent::PlanRequest,
            Intent::OrchestrateRequest,
            // DeepAnalyze outranks Brainstorm: "深度分析" requests heavy
            // reasoning, not open-ended ideation.
            Intent::DeepAnalyze,
            Intent::Brainstorm,
            Intent::DailyDigest,
            // Approval/Reject are evaluated last so create-* keywords still win
            // when a user says "approve creating a new skill called …".
            Intent::ApproveRequest,
            Intent::RejectRequest,
        ];

        Self { keywords, priority }
    }

    /// Classify a free-form prompt against the keyword set. Returns the first
    /// matching `Intent` per priority order, along with the matched keyword
    /// for provenance. Returns `None` when nothing matches.
    pub fn classify(&self, prompt: &str) -> Option<IntentMatch> {
        let lower = prompt.to_lowercase();
        for intent in &self.priority {
            if let Some(keywords) = self.keywords.get(intent) {
                for kw in keywords {
                    if lower.contains(kw) {
                        return Some(IntentMatch {
                            intent: *intent,
                            keyword: kw.clone(),
                        });
                    }
                }
            }
        }
        None
    }
}

impl Default for IntentClassifier {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_skill_english() {
        let c = IntentClassifier::with_defaults();
        let m = c
            .classify("Please create a skill that validates JSON")
            .unwrap();
        assert_eq!(m.intent, Intent::CreateSkill);
    }

    #[test]
    fn create_skill_chinese() {
        let c = IntentClassifier::with_defaults();
        let m = c.classify("帮我做一个 skill 负责校验 JSON").unwrap();
        assert_eq!(m.intent, Intent::CreateSkill);
    }

    #[test]
    fn create_agent_chinese() {
        let c = IntentClassifier::with_defaults();
        let m = c.classify("创建智能体叫爱马仕").unwrap();
        assert_eq!(m.intent, Intent::CreateAgent);
    }

    #[test]
    fn brainstorm_english() {
        let c = IntentClassifier::with_defaults();
        let m = c
            .classify("Let's brainstorm how to refactor memory layer")
            .unwrap();
        assert_eq!(m.intent, Intent::Brainstorm);
    }

    #[test]
    fn daily_digest_chinese() {
        let c = IntentClassifier::with_defaults();
        let m = c.classify("看一下今天做了什么").unwrap();
        assert_eq!(m.intent, Intent::DailyDigest);
    }

    #[test]
    fn no_match_returns_none() {
        let c = IntentClassifier::with_defaults();
        assert!(c.classify("what's the weather today").is_none());
    }

    #[test]
    fn case_insensitive() {
        let c = IntentClassifier::with_defaults();
        let m = c.classify("CREATE A SKILL FOR ME").unwrap();
        assert_eq!(m.intent, Intent::CreateSkill);
    }

    #[test]
    fn priority_create_skill_over_brainstorm() {
        // Prompt mentions both; CreateSkill takes priority.
        let c = IntentClassifier::with_defaults();
        let m = c
            .classify("Brainstorm and then create a skill for JSON parsing")
            .unwrap();
        assert_eq!(m.intent, Intent::CreateSkill);
    }

    // ── Sprint 12 L1 — approval intent coverage ───────────────────────────

    #[test]
    fn approve_request_english() {
        let c = IntentClassifier::with_defaults();
        let m = c.classify("approve request rv_123").unwrap();
        assert_eq!(m.intent, Intent::ApproveRequest);
    }

    #[test]
    fn approve_request_chinese() {
        let c = IntentClassifier::with_defaults();
        let m = c.classify("同意执行这个操作").unwrap();
        assert_eq!(m.intent, Intent::ApproveRequest);
    }

    #[test]
    fn reject_request_english() {
        let c = IntentClassifier::with_defaults();
        let m = c.classify("reject this capability").unwrap();
        assert_eq!(m.intent, Intent::RejectRequest);
    }

    #[test]
    fn reject_request_chinese() {
        let c = IntentClassifier::with_defaults();
        let m = c.classify("我拒绝这个请求").unwrap();
        assert_eq!(m.intent, Intent::RejectRequest);
    }

    #[test]
    fn create_skill_beats_approve_when_both_present() {
        // "create a skill" must win even if the sentence also contains
        // "approve" — create-* intents carry concrete side-effects.
        let c = IntentClassifier::with_defaults();
        let m = c
            .classify("please approve and create a skill for PR labels")
            .unwrap();
        assert_eq!(m.intent, Intent::CreateSkill);
    }

    // ── OMC Hybrid-C — PlanRequest coverage ───────────────────────────────

    #[test]
    fn plan_request_english_make_a_plan() {
        let c = IntentClassifier::with_defaults();
        let m = c
            .classify("Make a plan for the next sprint please")
            .unwrap();
        assert_eq!(m.intent, Intent::PlanRequest);
        assert_eq!(m.intent.target_facade(), "planner");
    }

    #[test]
    fn plan_request_english_break_down() {
        let c = IntentClassifier::with_defaults();
        let m = c
            .classify("Can you break this down into smaller tasks?")
            .unwrap();
        assert_eq!(m.intent, Intent::PlanRequest);
    }

    #[test]
    fn plan_request_chinese_guihua() {
        let c = IntentClassifier::with_defaults();
        let m = c.classify("帮我规划一下这个季度的交付").unwrap();
        assert_eq!(m.intent, Intent::PlanRequest);
    }

    #[test]
    fn plan_request_chinese_chaijie() {
        let c = IntentClassifier::with_defaults();
        let m = c.classify("把这个需求拆解一下").unwrap();
        assert_eq!(m.intent, Intent::PlanRequest);
    }

    #[test]
    fn plan_request_negative_not_every_plan_word() {
        // Bare "plan" / "the plan" must NOT trigger, to avoid matching
        // "floor plan", "plan on", "per plan", etc.
        let c = IntentClassifier::with_defaults();
        assert!(c.classify("what is the plan for today").is_none());
        assert!(c.classify("I plan on finishing soon").is_none());
        assert!(c.classify("look at the floor plan").is_none());
    }

    // ── OMC Hybrid-C — OrchestrateRequest coverage ────────────────────────

    #[test]
    fn orchestrate_request_english_multi_agent() {
        let c = IntentClassifier::with_defaults();
        let m = c
            .classify("spin up a multi-agent swarm for this migration")
            .unwrap();
        assert_eq!(m.intent, Intent::OrchestrateRequest);
        assert_eq!(m.intent.target_facade(), "orchestrator");
    }

    #[test]
    fn orchestrate_request_english_dispatch_parallel() {
        let c = IntentClassifier::with_defaults();
        let m = c
            .classify("dispatch parallel workers across the codebase")
            .unwrap();
        assert_eq!(m.intent, Intent::OrchestrateRequest);
    }

    #[test]
    fn orchestrate_request_chinese_bingxing() {
        let c = IntentClassifier::with_defaults();
        let m = c.classify("并行执行这三个子任务").unwrap();
        assert_eq!(m.intent, Intent::OrchestrateRequest);
    }

    #[test]
    fn orchestrate_request_chinese_coordinate_agents() {
        let c = IntentClassifier::with_defaults();
        let m = c.classify("协调多个 agent 一起干活").unwrap();
        assert_eq!(m.intent, Intent::OrchestrateRequest);
    }

    #[test]
    fn orchestrate_request_negative() {
        let c = IntentClassifier::with_defaults();
        // "agent" alone is not enough; "parallel" alone is not enough.
        assert!(c.classify("tell me about the agent model").is_none());
        assert!(c.classify("these lines are parallel").is_none());
        assert!(c.classify("单个 agent 就够了").is_none());
    }

    // ── OMC Hybrid-C — SkillifyRequest coverage ───────────────────────────

    #[test]
    fn skillify_request_english_save_as_skill() {
        let c = IntentClassifier::with_defaults();
        let m = c
            .classify("this worked well, save as a skill please")
            .unwrap();
        assert_eq!(m.intent, Intent::SkillifyRequest);
        assert_eq!(m.intent.target_facade(), "skill_creator");
    }

    #[test]
    fn skillify_request_english_skillify_keyword() {
        let c = IntentClassifier::with_defaults();
        let m = c.classify("please skillify this workflow").unwrap();
        assert_eq!(m.intent, Intent::SkillifyRequest);
    }

    #[test]
    fn skillify_request_chinese_chendian() {
        let c = IntentClassifier::with_defaults();
        let m = c.classify("把这个流程沉淀一下，方便以后复用").unwrap();
        assert_eq!(m.intent, Intent::SkillifyRequest);
    }

    #[test]
    fn skillify_request_chinese_zuocheng() {
        let c = IntentClassifier::with_defaults();
        let m = c.classify("把这流程做成 skill 吧").unwrap();
        assert_eq!(m.intent, Intent::SkillifyRequest);
    }

    #[test]
    fn skillify_beats_create_skill_when_both_match() {
        // "save this as a skill" is strictly more specific than the generic
        // CreateSkill authoring request — SkillifyRequest must win.
        let c = IntentClassifier::with_defaults();
        let m = c
            .classify("create a skill from this — save as a skill for the team")
            .unwrap();
        assert_eq!(m.intent, Intent::SkillifyRequest);
    }

    // ── OMC Hybrid-C — DeepAnalyze coverage ───────────────────────────────

    #[test]
    fn deep_analyze_english_ultrathink() {
        let c = IntentClassifier::with_defaults();
        let m = c
            .classify("ultrathink this architecture before we ship")
            .unwrap();
        assert_eq!(m.intent, Intent::DeepAnalyze);
        assert_eq!(m.intent.target_facade(), "deep_analyzer");
    }

    #[test]
    fn deep_analyze_english_deep_analyze() {
        let c = IntentClassifier::with_defaults();
        let m = c
            .classify("deep-analyze the failure mode of the retry loop")
            .unwrap();
        assert_eq!(m.intent, Intent::DeepAnalyze);
    }

    #[test]
    fn deep_analyze_chinese_shendu() {
        let c = IntentClassifier::with_defaults();
        let m = c.classify("对这个设计做一次深度分析").unwrap();
        assert_eq!(m.intent, Intent::DeepAnalyze);
    }

    #[test]
    fn deep_analyze_chinese_shenru() {
        let c = IntentClassifier::with_defaults();
        let m = c.classify("深入思考一下权衡取舍").unwrap();
        assert_eq!(m.intent, Intent::DeepAnalyze);
    }

    #[test]
    fn deep_analyze_beats_brainstorm() {
        // "deep-analyze" must outrank "brainstorm" in the same sentence, since
        // the former carries a concrete reasoning-mode side-effect.
        let c = IntentClassifier::with_defaults();
        let m = c
            .classify("brainstorm options then deep-analyze the top candidate")
            .unwrap();
        assert_eq!(m.intent, Intent::DeepAnalyze);
    }

    // ── target_facade() routing contract ──────────────────────────────────

    #[test]
    fn target_facade_stable_identifiers() {
        // Pin the facade identifiers so downstream routers can match on them
        // without coupling to the Intent enum layout.
        assert_eq!(Intent::CreateSkill.target_facade(), "skill_creator");
        assert_eq!(Intent::SkillifyRequest.target_facade(), "skill_creator");
        assert_eq!(Intent::CreateAgent.target_facade(), "agent_creator");
        assert_eq!(Intent::Brainstorm.target_facade(), "brainstormer");
        assert_eq!(Intent::DailyDigest.target_facade(), "daily_digest");
        assert_eq!(Intent::PlanRequest.target_facade(), "planner");
        assert_eq!(Intent::OrchestrateRequest.target_facade(), "orchestrator");
        assert_eq!(Intent::DeepAnalyze.target_facade(), "deep_analyzer");
        assert_eq!(Intent::ApproveRequest.target_facade(), "approval_gateway");
        assert_eq!(Intent::RejectRequest.target_facade(), "approval_gateway");
    }
}
