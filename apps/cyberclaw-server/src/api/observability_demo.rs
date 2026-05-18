//! Observability endpoints — backed by real audit + skill-hub data.
//!
//! These endpoints back the SPA admin views previously wired to
//! `MOCK_*` constants in `web/src/pages_c.jsx` and `web/src/pages_b.jsx`.
//! They now query real subsystems:
//!
//! | Method | Route | Source |
//! |---|---|---|
//! | GET | `/api/v1/runtime/timeline` | `AuditSink::tail_rows` (governance-relevant rows) |
//! | GET | `/api/v1/security/injection/hits` | `AuditSink::tail_rows` filtered to sanitizer entries |
//! | GET | `/api/v1/skills/poison-verdicts` | `SkillHub::list_quarantined` |
//!
//! Routes are registered *before* the `/api/v1/skills/:id` glob in
//! `lib.rs` so `poison-verdicts` is not captured as a `:id` segment.
//!
//! When the corresponding emitters land (live governance-decision
//! audit trail, sanitizer event recorder, signed verifier verdicts),
//! the same shapes flow through — only the row→DTO mapping changes.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::audit::{AuditKind, AuditQuery, AuditResult, AuditRow};
use crate::error::ApiError;
use crate::state::AppState;

/// Build the observability-demo router.
pub fn create_observability_demo_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/runtime/timeline", get(runtime_timeline))
        .route("/api/v1/security/injection/hits", get(injection_hits))
        .route("/api/v1/skills/poison-verdicts", get(poison_verdicts))
}

// ---------------------------------------------------------------------------
// Runtime timeline — recent governance-relevant audit rows.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct RuntimeEvent {
    id: String,
    rule: String,
    severity: String,
    action: String,
    actor: String,
    trace_id: String,
    ts: i64,
    offset_h: f64,
}

#[derive(Serialize)]
struct RuntimeTimelineResponse {
    events: Vec<RuntimeEvent>,
}

/// Optional query parameters for `/api/v1/runtime/timeline`.
#[derive(Debug, Default, Deserialize)]
struct TimelineQuery {
    /// Filter events to only those whose `trace_id` starts with this value.
    trace_id: Option<String>,
}

/// `GET /api/v1/runtime/timeline[?trace_id=X]` — recent governance-related decisions.
///
/// Sourced from the audit log. Pulls the most recent 40 `Mutation` and
/// `Security` rows, keeps only those whose action prefix is governance-
/// relevant (`chat.`, `agent.`, `capability.`, `sanitizer.`, `policy.`,
/// `governance.`), then surfaces the top 20 by timestamp.
///
/// When `trace_id` is provided, only events whose computed `trace_id`
/// field (first 12 chars of `this_hash`) starts with the given value
/// are returned.
async fn runtime_timeline(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TimelineQuery>,
) -> Result<Json<RuntimeTimelineResponse>, ApiError> {
    let Some(sink) = state.audit.as_ref() else {
        return Ok(Json(RuntimeTimelineResponse { events: vec![] }));
    };

    let mut rows: Vec<AuditRow> = Vec::new();
    for kind in [AuditKind::Mutation, AuditKind::Security] {
        let q = AuditQuery {
            kind: Some(kind),
            ..Default::default()
        };
        match sink.tail_rows(40, &q).await {
            Ok(mut r) => rows.append(&mut r),
            Err(e) => {
                tracing::warn!(error=%e, "audit tail_rows failed for runtime_timeline");
            }
        }
    }

    rows.sort_by_key(|r| std::cmp::Reverse(r.entry.ts));
    rows.dedup_by(|a, b| a.id == b.id);

    let now = chrono::Utc::now().timestamp_millis();
    let trace_filter = params.trace_id.as_deref().unwrap_or("").to_string();

    let events: Vec<RuntimeEvent> = rows
        .into_iter()
        .filter(|row| is_governance_relevant(&row.entry.action))
        .map(|row| {
            let ts_ms = row.entry.ts.timestamp_millis();
            let offset_h = ((now - ts_ms) as f64) / 3_600_000.0;
            let action_label = result_label(&row.entry.result).to_string();
            let severity = severity_for(&row.entry.action, &action_label);
            let event_trace_id: String = row.this_hash.chars().take(12).collect();
            RuntimeEvent {
                id: format!("re_{}", row.id),
                rule: row.entry.action.clone(),
                severity,
                action: action_label,
                actor: row.entry.actor.clone(),
                trace_id: event_trace_id,
                ts: ts_ms,
                offset_h,
            }
        })
        .filter(|ev| trace_filter.is_empty() || ev.trace_id.starts_with(&trace_filter))
        .take(20)
        .collect();

    Ok(Json(RuntimeTimelineResponse { events }))
}

fn result_label(r: &AuditResult) -> &'static str {
    match r {
        AuditResult::Success => "success",
        AuditResult::Failure { .. } => "failure",
    }
}

fn is_governance_relevant(action: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "chat.",
        "agent.",
        "capability.",
        "sanitizer.",
        "policy.",
        "governance.",
        "permission.",
        "review.",
        "reviews.",
    ];
    PREFIXES.iter().any(|p| action.starts_with(p))
}

fn severity_for(action: &str, result_label: &str) -> String {
    let a = action.to_lowercase();
    if a.contains("denied") || a.contains("blocked") || result_label == "failure" {
        "high".into()
    } else if a.contains("ask")
        || a.contains("review")
        || a.contains("warn")
        || a.contains("sanitizer")
        || a.contains("policy")
    {
        "medium".into()
    } else {
        "low".into()
    }
}

// ---------------------------------------------------------------------------
// Injection hits — recent sanitizer / credential-leak audit rows.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct InjectionHit {
    id: String,
    severity: String,
    pattern: String,
    actor: String,
    trace_id: String,
    rule_id: String,
    ts: i64,
}

#[derive(Serialize)]
struct InjectionHitsResponse {
    hits: Vec<InjectionHit>,
}

/// `GET /api/v1/security/injection/hits` — sanitizer hits surfaced from
/// the audit log. Returns rows whose `kind = Security` and `action`
/// starts with `sanitizer.`. Returns an empty list when no such entries
/// exist (honest-empty rather than canned data).
async fn injection_hits(
    State(state): State<Arc<AppState>>,
) -> Result<Json<InjectionHitsResponse>, ApiError> {
    let Some(sink) = state.audit.as_ref() else {
        return Ok(Json(InjectionHitsResponse { hits: vec![] }));
    };

    let q = AuditQuery {
        kind: Some(AuditKind::Security),
        action_prefix: Some("sanitizer.".into()),
        ..Default::default()
    };
    let rows = sink
        .tail_rows(50, &q)
        .await
        .map_err(|e| ApiError::InternalError(format!("audit tail_rows failed: {e}")))?;

    let hits: Vec<InjectionHit> = rows
        .into_iter()
        .map(|row| {
            let detail = &row.entry.detail;
            let pattern = detail
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or(&row.entry.action)
                .to_string();
            let severity = detail
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| match result_label(&row.entry.result) {
                    "failure" => "HIGH",
                    _ => "MED",
                })
                .to_string();
            InjectionHit {
                id: format!("ih_{}", row.id),
                severity,
                pattern,
                actor: row.entry.actor.clone(),
                trace_id: row.this_hash.chars().take(12).collect(),
                rule_id: row.entry.action.clone(),
                ts: row.entry.ts.timestamp_millis(),
            }
        })
        .collect();

    Ok(Json(InjectionHitsResponse { hits }))
}

// ---------------------------------------------------------------------------
// Poison verdicts — quarantined skills from SkillHub.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ScanReport {
    scanner_version: String,
    patterns_matched: Vec<String>,
    risk_score: f32,
}

#[derive(Serialize)]
struct PoisonVerdict {
    skill_id: String,
    name: String,
    submitter: String,
    submitted_at: String,
    verdict: String,
    trust_level: String,
    signature_valid: bool,
    findings: Vec<String>,
    scan_report: ScanReport,
}

#[derive(Serialize)]
struct PoisonVerdictsResponse {
    verdicts: Vec<PoisonVerdict>,
}

/// `GET /api/v1/skills/poison-verdicts` — quarantined skills.
///
/// Backed by `SkillHub::list_quarantined()`. A bundle in the quarantine
/// directory implies the scanner returned `Block` or `RequiresReview`
/// (see `SkillHub::scan_and_install`). Until the scanner persists its
/// findings to disk per-bundle, fields below are derived from the
/// bundle metadata only.
async fn poison_verdicts(
    State(state): State<Arc<AppState>>,
) -> Result<Json<PoisonVerdictsResponse>, ApiError> {
    let bundles = state.skill_hub.read().await.list_quarantined();
    let now = chrono::Utc::now().to_rfc3339();
    let verdicts = bundles
        .into_iter()
        .map(|b| {
            let trust_level = match b.trust_level {
                cyberclaw_skill_runtime::skill_scanner::SkillTrustLevel::Builtin => "builtin",
                cyberclaw_skill_runtime::skill_scanner::SkillTrustLevel::Trusted => "trusted",
                cyberclaw_skill_runtime::skill_scanner::SkillTrustLevel::Community => "community",
                cyberclaw_skill_runtime::skill_scanner::SkillTrustLevel::AgentCreated => {
                    "agent-created"
                }
            };
            PoisonVerdict {
                skill_id: format!("sk_q_{}", b.name),
                name: b.name.clone(),
                submitter: b
                    .publisher_fingerprint
                    .clone()
                    .unwrap_or_else(|| "unknown".into()),
                submitted_at: now.clone(),
                verdict: "Quarantine".into(),
                trust_level: trust_level.into(),
                signature_valid: b.signature.is_some(),
                findings: vec![],
                scan_report: ScanReport {
                    scanner_version: env!("CARGO_PKG_VERSION").into(),
                    patterns_matched: vec![],
                    risk_score: 0.0,
                },
            }
        })
        .collect();

    Ok(Json(PoisonVerdictsResponse { verdicts }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_helpers::build_test_state;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn runtime_timeline_returns_events() {
        let state = build_test_state();
        let app = Router::new()
            .route("/api/v1/runtime/timeline", get(runtime_timeline))
            .with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/runtime/timeline")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn runtime_timeline_trace_id_filter() {
        let state = build_test_state();
        let app = Router::new()
            .route("/api/v1/runtime/timeline", get(runtime_timeline))
            .with_state(state);
        // Filter by a trace_id that cannot match any hash — expect empty events.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/runtime/timeline?trace_id=nonexistent000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["events"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn injection_hits_returns_hits() {
        let state = build_test_state();
        let app = Router::new()
            .route("/api/v1/security/injection/hits", get(injection_hits))
            .with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/security/injection/hits")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[test]
    fn is_governance_relevant_accepts_known_prefixes() {
        // These are the prefixes that already exist in the audit log
        // (search apps/cyberclaw-server/src/api/*.rs for AuditEntry::now).
        // If a new prefix is introduced and forgotten here, the runtime
        // timeline panel silently drops the event.
        let must_pass = [
            "chat.intent_hint",
            "chat.auto_routed:create_skill",
            "agent.create",
            "agent.invoke",
            "capability.dispatch",
            "sanitizer.prompt_injection",
            "sanitizer.credential_leak",
            "policy.review_required",
            "governance.deny",
            "permission.allow",
            "review.approved",
            "reviews.created",
        ];
        for action in must_pass {
            assert!(
                is_governance_relevant(action),
                "{action} should be governance-relevant"
            );
        }
    }

    #[test]
    fn is_governance_relevant_rejects_unrelated() {
        // Generic / debug actions shouldn't pollute the runtime timeline.
        let must_fail = [
            "user.profile.update",
            "session.refresh",
            "metrics.scrape",
            "health.ping",
            "",
        ];
        for action in must_fail {
            assert!(
                !is_governance_relevant(action),
                "{action} should NOT be governance-relevant"
            );
        }
    }

    #[test]
    fn severity_for_classifies_action_keywords() {
        assert_eq!(severity_for("capability.denied", "success"), "high");
        assert_eq!(severity_for("policy.blocked", "success"), "high");
        assert_eq!(severity_for("agent.invoke", "failure"), "high");
        assert_eq!(severity_for("review.required", "success"), "medium");
        assert_eq!(severity_for("permission.ask", "success"), "medium");
        assert_eq!(
            severity_for("sanitizer.prompt_injection", "success"),
            "medium"
        );
        assert_eq!(severity_for("agent.create", "success"), "low");
    }

    #[tokio::test]
    async fn poison_verdicts_returns_verdicts() {
        let state = build_test_state();
        let app = Router::new()
            .route("/api/v1/skills/poison-verdicts", get(poison_verdicts))
            .with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/skills/poison-verdicts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }
}
