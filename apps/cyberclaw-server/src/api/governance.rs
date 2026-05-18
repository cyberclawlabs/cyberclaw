//! Governance / Reviews aggregate endpoints (Sprint 11 W2 L1).
//!
//! Replaces the frontend mock TODOs in `web/src/pages_c.jsx` §Reviews and
//! §Security. Every route in this module is mounted on the authenticated
//! lane and sensitive writes (PATCH) additionally require `admin` role.
//!
//! # Routes
//!
//! | Method | Route | Auth | Purpose |
//! |---|---|---|---|
//! | GET | `/api/v1/reviews/audit-agent/profile` | JWT | Aggregated Audit Agent self-evolution profile |
//! | GET | `/api/v1/reviews/audit-agent/accuracy-curve?days=30` | JWT | Sparkline: daily accuracy points |
//! | GET | `/api/v1/security/permission/rules` | JWT | 31 default rules (DCF + ToolPermissionMatcher) |
//! | PATCH | `/api/v1/security/permission/rules/:id` | JWT + admin | Queue a mutation proposal into the review queue |
//!
//! # Data sources
//!
//! - Profile + accuracy curve: derived from `AuditSink` rows (kind=mutation,
//!   action prefix `review.`). Sprint 10 does not yet ship a true Audit
//!   Agent learning profile, so "accuracy" is defined as
//!   `approved_count / (approved_count + rejected_count)` over the window.
//! - Permission rules: `DangerousCapabilityFilter::with_defaults()` (7) +
//!   `ToolPermissionMatcher::default()` (24) = 31 rules shown read-only in
//!   the UI; see `/admin` console, `pages_c.jsx` §1059.
//! - PATCH proposals: appended to `AppState.permission_rule_proposals`
//!   (in-memory) and a `security.rule.*` audit row. Operators resolve
//!   proposals from the normal `/reviews` surface.

use std::sync::Arc;

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    routing::{get, patch},
    Json, Router,
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use tracing::info;

use cyberclaw_governance::dangerous_capability_filter::{
    DangerSeverity, DangerousCapabilityFilter,
};
use cyberclaw_governance::tool_permission_matcher::{PermissionDecision, ToolPermissionMatcher};

use crate::audit::{AuditEntry, AuditKind, AuditQuery, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::{require_admin, Claims};
use crate::state::AppState;

/// Build the governance/reviews router.
pub fn create_governance_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/reviews/audit-agent/profile",
            get(audit_agent_profile),
        )
        .route(
            "/api/v1/reviews/audit-agent/accuracy-curve",
            get(audit_agent_accuracy_curve),
        )
        .route(
            "/api/v1/security/permission/rules",
            get(list_permission_rules),
        )
        .route(
            "/api/v1/security/permission/rules/:id",
            patch(patch_permission_rule),
        )
        .route("/api/v1/governance/policy-rules", get(list_policy_rules))
        .route("/api/v1/governance/iron-law", get(list_iron_laws))
}

// ---------------------------------------------------------------------------
// Audit Agent profile + accuracy curve
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct LearnedPattern {
    pattern: String,
    count: u32,
    last_seen: String,
}

#[derive(Debug, Serialize)]
struct AuditAgentProfile {
    agent_id: String,
    accuracy: f64,
    total_decisions: u32,
    auto_approved: u32,
    manual_override: u32,
    learned_patterns: Vec<LearnedPattern>,
}

/// `GET /api/v1/reviews/audit-agent/profile`
///
/// Aggregates audit-log rows where `kind = mutation` and `action` starts
/// with `review.` to approximate an audit-agent decision profile. When the
/// audit sink is absent (test build), returns zeros instead of failing.
async fn audit_agent_profile(
    State(state): State<Arc<AppState>>,
) -> Result<Json<AuditAgentProfile>, ApiError> {
    info!("governance: building audit-agent profile");

    let sink = match state.audit.as_ref() {
        Some(s) => s.clone(),
        None => {
            return Ok(Json(AuditAgentProfile {
                agent_id: "internal.audit.governor".to_string(),
                accuracy: 0.0,
                total_decisions: 0,
                auto_approved: 0,
                manual_override: 0,
                learned_patterns: Vec::new(),
            }))
        }
    };

    // Pull the last 1000 review-related rows.
    let filters = AuditQuery {
        kind: Some(AuditKind::Mutation),
        action_prefix: Some("review.".to_string()),
        ..Default::default()
    };
    let rows = sink
        .tail(1000, &filters)
        .await
        .map_err(|e| ApiError::InternalError(format!("audit tail failed: {}", e)))?;

    let mut approved = 0u32;
    let mut rejected = 0u32;
    let mut auto_approved = 0u32;
    let mut manual_override = 0u32;
    let mut pattern_counts: std::collections::HashMap<String, (u32, String)> =
        std::collections::HashMap::new();

    for entry in &rows {
        let action = entry.action.as_str();
        let ts = entry.ts.to_rfc3339();
        let pattern_key = action.to_string();
        let slot = pattern_counts.entry(pattern_key).or_insert((0, ts.clone()));
        slot.0 += 1;
        slot.1 = ts;

        match action {
            "review.approve" => approved += 1,
            "review.reject" => rejected += 1,
            _ => {}
        }
        if entry.actor == "system" || entry.actor == "internal.audit.governor" {
            auto_approved += 1;
        } else {
            manual_override += 1;
        }
    }

    let total_decisions = approved + rejected;
    let accuracy = if total_decisions == 0 {
        0.0
    } else {
        approved as f64 / total_decisions as f64
    };

    let mut learned_patterns: Vec<LearnedPattern> = pattern_counts
        .into_iter()
        .map(|(pattern, (count, last_seen))| LearnedPattern {
            pattern,
            count,
            last_seen,
        })
        .collect();
    learned_patterns.sort_by_key(|b| std::cmp::Reverse(b.count));

    Ok(Json(AuditAgentProfile {
        agent_id: "internal.audit.governor".to_string(),
        accuracy,
        total_decisions,
        auto_approved,
        manual_override,
        learned_patterns,
    }))
}

#[derive(Debug, Deserialize)]
pub struct AccuracyCurveQuery {
    /// Lookback window in days. Default 30; clamped to [1, 365].
    pub days: Option<u32>,
}

#[derive(Debug, Serialize)]
struct AccuracyPoint {
    /// ISO date `YYYY-MM-DD`.
    date: String,
    accuracy: f64,
}

#[derive(Debug, Serialize)]
struct AccuracyCurveResponse {
    points: Vec<AccuracyPoint>,
}

/// `GET /api/v1/reviews/audit-agent/accuracy-curve?days=30`
///
/// Bucketizes review audit rows by day and computes `approved / (approved +
/// rejected)` per bucket. Missing days are returned with `accuracy = 0.0`
/// so the sparkline can render a continuous x-axis.
async fn audit_agent_accuracy_curve(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AccuracyCurveQuery>,
) -> Result<Json<AccuracyCurveResponse>, ApiError> {
    let days = query.days.unwrap_or(30).clamp(1, 365) as i64;
    let now = Utc::now();
    let since = now - ChronoDuration::days(days);

    let sink = match state.audit.as_ref() {
        Some(s) => s.clone(),
        None => {
            return Ok(Json(AccuracyCurveResponse {
                points: empty_curve(since, days),
            }))
        }
    };

    let filters = AuditQuery {
        kind: Some(AuditKind::Mutation),
        action_prefix: Some("review.".to_string()),
        since: Some(since),
        ..Default::default()
    };
    let rows = sink
        .tail(5000, &filters)
        .await
        .map_err(|e| ApiError::InternalError(format!("audit tail failed: {}", e)))?;

    let mut buckets: std::collections::BTreeMap<String, (u32, u32)> =
        std::collections::BTreeMap::new();
    for entry in &rows {
        let date = entry.ts.date_naive().to_string();
        let slot = buckets.entry(date).or_insert((0, 0));
        match entry.action.as_str() {
            "review.approve" => slot.0 += 1,
            "review.reject" => slot.1 += 1,
            _ => {}
        }
    }

    // Fill in every day in the window so the sparkline has a stable x-axis.
    let mut points = Vec::with_capacity(days as usize);
    for offset in 0..days {
        let day = (since + ChronoDuration::days(offset))
            .date_naive()
            .to_string();
        let (approved, rejected) = buckets.get(&day).copied().unwrap_or((0, 0));
        let total = approved + rejected;
        let accuracy = if total == 0 {
            0.0
        } else {
            approved as f64 / total as f64
        };
        points.push(AccuracyPoint {
            date: day,
            accuracy,
        });
    }

    Ok(Json(AccuracyCurveResponse { points }))
}

fn empty_curve(since: DateTime<Utc>, days: i64) -> Vec<AccuracyPoint> {
    (0..days)
        .map(|offset| AccuracyPoint {
            date: (since + ChronoDuration::days(offset))
                .date_naive()
                .to_string(),
            accuracy: 0.0,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Permission rules (DangerousCapabilityFilter + ToolPermissionMatcher)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct PermissionRuleDto {
    id: String,
    /// `"dcf"` (DangerousCapabilityFilter) or `"tpm"` (ToolPermissionMatcher).
    source: &'static str,
    pattern: String,
    action: &'static str,
    severity: &'static str,
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct ListRulesResponse {
    rules: Vec<PermissionRuleDto>,
}

/// `GET /api/v1/security/permission/rules`
///
/// Materializes the default rule set from governance's two rule engines.
/// Returned as a flat list keyed by synthetic ids so the admin UI can
/// PATCH an individual rule via `/api/v1/security/permission/rules/:id`.
async fn list_permission_rules() -> Result<Json<ListRulesResponse>, ApiError> {
    let dcf = DangerousCapabilityFilter::with_defaults();
    let tpm = ToolPermissionMatcher::default();

    let mut rules: Vec<PermissionRuleDto> = Vec::new();
    for r in dcf.rules() {
        rules.push(PermissionRuleDto {
            id: format!("dcf:{}", r.id),
            source: "dcf",
            pattern: r.capability_pattern.clone(),
            action: match r.severity {
                DangerSeverity::Critical | DangerSeverity::High => "deny",
                DangerSeverity::Medium => "warn",
            },
            severity: match r.severity {
                DangerSeverity::Critical => "critical",
                DangerSeverity::High => "high",
                DangerSeverity::Medium => "medium",
            },
            enabled: true,
        });
    }
    for (idx, r) in tpm.rules().iter().enumerate() {
        rules.push(PermissionRuleDto {
            id: format!("tpm:{}", idx),
            source: "tpm",
            pattern: match &r.argument_pattern {
                Some(arg) => format!("{} {}", r.tool_pattern, arg),
                None => r.tool_pattern.clone(),
            },
            action: match r.decision {
                PermissionDecision::Allow => "allow",
                PermissionDecision::Ask => "ask",
                PermissionDecision::Deny => "deny",
            },
            severity: match r.decision {
                PermissionDecision::Deny => "high",
                PermissionDecision::Ask => "medium",
                PermissionDecision::Allow => "low",
            },
            enabled: true,
        });
    }

    Ok(Json(ListRulesResponse { rules }))
}

#[derive(Debug, Deserialize)]
pub struct PatchRuleRequest {
    /// New action: `"allow" | "ask" | "deny" | "warn"`.
    #[serde(default)]
    pub action: Option<String>,
    /// Toggle on/off (soft disable).
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Operator-facing justification (stored with the proposal).
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PermissionRuleProposal {
    pub proposal_id: String,
    pub rule_id: String,
    pub action: Option<String>,
    pub enabled: Option<bool>,
    pub reason: Option<String>,
    pub proposed_by: String,
    pub proposed_at: String,
    pub status: &'static str,
}

/// `PATCH /api/v1/security/permission/rules/:id`
///
/// Never mutates the live rule set. Appends a proposal to
/// `AppState.permission_rule_proposals` and records an audit row. The
/// operator approves or rejects the proposal via the normal
/// `/api/v1/reviews` surface. This matches the UX contract in
/// `pages_c.jsx:303`: "enters /reviews approval flow".
async fn patch_permission_rule(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(rule_id): Path<String>,
    Json(req): Json<PatchRuleRequest>,
) -> Result<(StatusCode, Json<PermissionRuleProposal>), ApiError> {
    if let Err(err) = require_admin(&claims).await {
        if let Some(sink) = state.audit.as_ref() {
            sink.record(AuditEntry::now(
                claims.sub.as_str().to_string(),
                AuditKind::Auth,
                "security.rule.patch:role_denied".to_string(),
                Some(format!("rule:{}", rule_id)),
                serde_json::json!({}),
                AuditResult::Failure {
                    reason: "admin role required".to_string(),
                },
            ))
            .await;
        }
        return Err(err);
    }

    // Validate known action labels if the caller sent one.
    if let Some(ref action) = req.action {
        if !matches!(action.as_str(), "allow" | "ask" | "deny" | "warn") {
            return Err(ApiError::InvalidInput(format!(
                "unknown action '{}'. expected one of allow|ask|deny|warn",
                action
            )));
        }
    }

    let proposal = PermissionRuleProposal {
        proposal_id: format!("prop_{}", uuid::Uuid::new_v4().simple()),
        rule_id: rule_id.clone(),
        action: req.action.clone(),
        enabled: req.enabled,
        reason: req.reason.clone(),
        proposed_by: claims.sub.as_str().to_string(),
        proposed_at: Utc::now().to_rfc3339(),
        status: "pending",
    };

    {
        let mut proposals = state.permission_rule_proposals.write().await;
        proposals.push(proposal.clone());
    }

    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            claims.sub.as_str().to_string(),
            AuditKind::Security,
            "security.rule.patch".to_string(),
            Some(format!("rule:{}", rule_id)),
            serde_json::json!({
                "proposal_id": proposal.proposal_id,
                "action": proposal.action,
                "enabled": proposal.enabled,
            }),
            AuditResult::Success,
        ))
        .await;
    }

    info!(
        proposal_id = %proposal.proposal_id,
        rule_id = %rule_id,
        "governance: permission rule patch proposed (queued for review)"
    );

    // 202 Accepted: proposal enqueued, not yet effective.
    Ok((StatusCode::ACCEPTED, Json(proposal)))
}

// ---------------------------------------------------------------------------
// S29 — Policy rules (RuleBasedPolicyEngine declarative RuleSet)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct PolicyRuleDto {
    /// `"deny"` | `"allow"`.
    kind: String,
    /// `null` means wildcard (match any agent).
    agent_id: Option<String>,
    /// `null` means wildcard (match any capability).
    capability_id: Option<String>,
    /// Sprint 30: numeric priority. 0 = file-order semantics.
    priority: i32,
    /// Operator-facing reason embedded in the EvaluationResult.
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct ListPolicyRulesResponse {
    /// `"rule_based"` when a `RuleBasedPolicyEngine` is wired,
    /// `"default"` (DefaultPolicyEngine), or `"unknown"` for any other engine.
    engine: &'static str,
    rules: Vec<PolicyRuleDto>,
}

/// `GET /api/v1/governance/policy-rules`
///
/// Sprint 29 read-only admin surface for the S27 declarative `RuleSet`. When
/// the active [`PolicyEngine`] is a [`RuleBasedPolicyEngine`], lists its live
/// rules (S28 hot-reload sees these update without restart). When the engine
/// is risk-based only ([`DefaultPolicyEngine`]), returns an empty array with
/// `engine = "default"` so the UI can show "no declarative rules configured".
async fn list_policy_rules(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ListPolicyRulesResponse>, ApiError> {
    use cyberclaw_governance::rules::RuleKind;

    match state.policy_engine.rules_snapshot() {
        Some(rs) => {
            let rules = rs
                .rules
                .iter()
                .map(|r| PolicyRuleDto {
                    kind: match r.kind {
                        RuleKind::Allow => "allow".to_string(),
                        RuleKind::Deny => "deny".to_string(),
                    },
                    agent_id: r.agent_id.clone(),
                    capability_id: r.capability_id.clone(),
                    priority: r.priority,
                    reason: r.reason.clone(),
                })
                .collect();
            Ok(Json(ListPolicyRulesResponse {
                engine: "rule_based",
                rules,
            }))
        }
        None => Ok(Json(ListPolicyRulesResponse {
            engine: "default",
            rules: Vec::new(),
        })),
    }
}

// ---------------------------------------------------------------------------
// Iron Law inspection (v1.0-rc4 — Constitutional transparency)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct RationalizationEntryDto {
    excuse: String,
    rebuttal: String,
}

#[derive(Debug, Serialize)]
struct IronLawDto {
    /// Rule name this Iron Law is attached to (e.g. `critical-chain-of-command`).
    rule: String,
    /// Numeric priority — higher = checked earlier in policy evaluation.
    priority: u32,
    /// The absolute non-negotiable declaration.
    declaration: String,
    /// Anti-rationalization table: excuse → rebuttal pairs.
    anti_rationalization: Vec<RationalizationEntryDto>,
    /// Self-check signals indicating imminent violation.
    red_flags: Vec<String>,
}

#[derive(Debug, Serialize)]
struct IronLawResponse {
    /// Schema version of this payload.
    version: &'static str,
    /// `true` when these laws are injected into the agent's prompt at runtime.
    injected_into_prompt: bool,
    /// How many ApprovalRules in the default policy carry an Iron Law.
    total: usize,
    /// The Iron Laws themselves, ordered by priority (high → low).
    laws: Vec<IronLawDto>,
}

/// `GET /api/v1/governance/iron-law`
///
/// Operator-facing introspection of the Constitutional AI layer. Returns every
/// Iron Law currently embedded in the default `ApprovalPolicy`, plus its
/// anti-rationalization table and red-flag signals.
///
/// This endpoint exists to make the governance layer **inspectable** rather
/// than buried in source — auditors can diff this payload across versions,
/// red teams can target gaps, and SOC2 reviewers can verify the constitutional
/// surface without reading Rust.
///
/// Note: this returns the *default* policy's laws — runtime hot-reloaded
/// rules will land here in a follow-up when `PolicyEngine::rules_snapshot`
/// is widened to carry Iron Law metadata.
async fn list_iron_laws(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<IronLawResponse>, ApiError> {
    use cyberclaw_governance::approval_policy::ApprovalPolicy;

    let policy = ApprovalPolicy::default_policy();
    let mut laws: Vec<IronLawDto> = policy
        .rules()
        .iter()
        .filter_map(|rule| {
            rule.iron_law.as_ref().map(|law| IronLawDto {
                rule: rule.name.clone(),
                priority: rule.priority,
                declaration: law.declaration.clone(),
                anti_rationalization: law
                    .rationalization_table
                    .iter()
                    .map(|e| RationalizationEntryDto {
                        excuse: e.excuse.clone(),
                        rebuttal: e.reality.clone(),
                    })
                    .collect(),
                red_flags: law.red_flags.clone(),
            })
        })
        .collect();
    laws.sort_by_key(|l| std::cmp::Reverse(l.priority));

    let total = laws.len();
    Ok(Json(IronLawResponse {
        version: "v1.0",
        injected_into_prompt: true,
        total,
        laws,
    }))
}

// ---------------------------------------------------------------------------
// Tests (happy-path coverage)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;
    use crate::api::test_helpers::{build_test_state, seed_users_with_role};
    use cyberclaw_core::ids::UserId;
    use serial_test::serial;

    fn claims_for(user_id: &str) -> Claims {
        let uid = UserId::from_string(user_id.to_string()).expect("valid user id");
        let now = Utc::now().timestamp();
        Claims {
            sub: uid,
            tenant: None,
            iat: now,
            exp: now + 3600,
        }
    }

    #[tokio::test]
    async fn permission_rules_contains_dcf_and_tpm_rules() {
        let resp = list_permission_rules().await.expect("list ok");
        let body = resp.0;
        // DCF ships 7 defaults + TPM ships 24 defaults = 31 rules.
        // Accept >=31 so adding a future rule doesn't break this test.
        assert!(
            body.rules.len() >= 31,
            "expected >= 31 rules, got {}",
            body.rules.len()
        );
        let has_dcf = body.rules.iter().any(|r| r.source == "dcf");
        let has_tpm = body.rules.iter().any(|r| r.source == "tpm");
        assert!(has_dcf && has_tpm, "must include both rule sources");
    }

    #[tokio::test]
    async fn audit_agent_profile_handles_missing_sink() {
        let state = build_test_state();
        let resp = audit_agent_profile(State(state)).await.expect("profile ok");
        // Test state has no audit sink; endpoint must return zeros, not 500.
        assert_eq!(resp.0.total_decisions, 0);
        assert_eq!(resp.0.accuracy, 0.0);
        assert_eq!(resp.0.agent_id, "internal.audit.governor");
    }

    #[tokio::test]
    async fn accuracy_curve_returns_point_per_day() {
        let state = build_test_state();
        let resp =
            audit_agent_accuracy_curve(State(state), Query(AccuracyCurveQuery { days: Some(7) }))
                .await
                .expect("curve ok");
        assert_eq!(resp.0.points.len(), 7, "should have 7 daily points");
    }

    #[tokio::test]
    #[serial]
    async fn patch_rule_rejects_viewer_role() {
        let (_tmp, _restore) = seed_users_with_role("op-viewer-rule", "viewer");
        let state = build_test_state();
        let claims = claims_for("op-viewer-rule");
        let err = patch_permission_rule(
            State(state),
            Extension(claims),
            Path("dcf:D001".to_string()),
            Json(PatchRuleRequest {
                action: Some("allow".to_string()),
                enabled: None,
                reason: Some("test".to_string()),
            }),
        )
        .await
        .expect_err("viewer must be denied");
        assert!(matches!(err, ApiError::Forbidden(_)));
    }

    #[tokio::test]
    #[serial]
    async fn patch_rule_admin_enqueues_proposal() {
        let (_tmp, _restore) = seed_users_with_role("op-admin-rule", "admin");
        let state = build_test_state();
        let claims = claims_for("op-admin-rule");
        let (code, Json(proposal)) = patch_permission_rule(
            State(state.clone()),
            Extension(claims),
            Path("dcf:D002".to_string()),
            Json(PatchRuleRequest {
                action: Some("ask".to_string()),
                enabled: Some(true),
                reason: Some("soften policy for CI".to_string()),
            }),
        )
        .await
        .expect("admin must pass");
        assert_eq!(code, StatusCode::ACCEPTED);
        assert_eq!(proposal.rule_id, "dcf:D002");
        assert_eq!(proposal.status, "pending");

        let proposals = state.permission_rule_proposals.read().await;
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].proposal_id, proposal.proposal_id);
    }

    #[tokio::test]
    async fn list_policy_rules_returns_empty_for_default_engine() {
        // S29: when AppState is built with the default engine (DefaultPolicyEngine),
        // rules_snapshot() returns None and the API exposes engine="default" + [].
        let state = build_test_state();
        let resp = list_policy_rules(State(state)).await.expect("ok");
        assert_eq!(resp.0.engine, "default");
        assert!(resp.0.rules.is_empty());
    }

    #[tokio::test]
    async fn list_policy_rules_returns_rules_for_rule_based_engine() {
        // S29: build an AppState whose policy_engine is a RuleBasedPolicyEngine
        // wrapping a 2-rule yaml. The API must surface both rules with their
        // priority/reason fields intact.
        use cyberclaw_governance::engine::{DefaultPolicyEngine, RuleBasedPolicyEngine};
        use cyberclaw_governance::rules::RuleSet;

        let yaml = r#"
rules:
  - kind: deny
    agent_id: junior
    capability_id: agent.handoff
    priority: 5
    reason: "junior agents cannot transfer"
  - kind: allow
    agent_id: trusted
    capability_id: null
    priority: 10
    reason: "trusted agent has explicit grant"
"#;
        let rs: RuleSet = RuleSet::from_yaml_str(yaml).expect("yaml parses");
        let engine: Arc<dyn cyberclaw_governance::engine::PolicyEngine> = Arc::new(
            RuleBasedPolicyEngine::new(rs, DefaultPolicyEngine::default()),
        );

        let mut state = build_test_state();
        // build_test_state returns Arc<AppState>; we need to swap policy_engine.
        // Since AppState fields are public, we rebuild via Arc::make_mut if
        // there's exactly one strong ref (true here — build_test_state is fresh).
        let mutable = Arc::get_mut(&mut state).expect("test state must be uniquely owned");
        mutable.policy_engine = engine;

        let resp = list_policy_rules(State(state)).await.expect("ok");
        assert_eq!(resp.0.engine, "rule_based");
        assert_eq!(resp.0.rules.len(), 2);

        let r0 = &resp.0.rules[0];
        assert_eq!(r0.kind, "deny");
        assert_eq!(r0.agent_id.as_deref(), Some("junior"));
        assert_eq!(r0.capability_id.as_deref(), Some("agent.handoff"));
        assert_eq!(r0.priority, 5);
        assert_eq!(r0.reason.as_deref(), Some("junior agents cannot transfer"));

        let r1 = &resp.0.rules[1];
        assert_eq!(r1.kind, "allow");
        assert_eq!(r1.agent_id.as_deref(), Some("trusted"));
        assert!(r1.capability_id.is_none(), "wildcard capability stays null");
        assert_eq!(r1.priority, 10);
    }

    #[tokio::test]
    #[serial]
    async fn patch_rule_rejects_unknown_action() {
        let (_tmp, _restore) = seed_users_with_role("op-admin-badaction", "admin");
        let state = build_test_state();
        let claims = claims_for("op-admin-badaction");
        let err = patch_permission_rule(
            State(state),
            Extension(claims),
            Path("dcf:D001".to_string()),
            Json(PatchRuleRequest {
                action: Some("nuke".to_string()),
                enabled: None,
                reason: None,
            }),
        )
        .await
        .expect_err("unknown action must 400");
        assert!(matches!(err, ApiError::InvalidInput(_)));
    }
}
