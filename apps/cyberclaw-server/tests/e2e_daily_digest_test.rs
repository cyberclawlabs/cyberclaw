//! Sprint 9 L9 — end-to-end test for the daily-digest lane.
//!
//! Flow verified:
//! 1. Seed an `Execution` into the shared `ExecutionService` (via
//!    `submit_plan` + `update_status`) with a specific agent id inside the
//!    lookback window.
//! 2. Run `DefaultDailyDigestCoordinator` wired with `StoreDigestCollector`
//!    and the server's `state.digest_repo` so both write and read go
//!    through the production path.
//! 3. Hit `GET /api/v1/agents/:id/digest?days=30` and assert the entry
//!    surfaces on the API.

#![allow(clippy::duplicate_mod)]

#[path = "common/mod.rs"]
mod common;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use chrono::{Duration, Utc};
use tower::ServiceExt;

use cyberclaw_control_plane::daily_digest::{
    DailyDigestConfig, DefaultDailyDigestCoordinator, DigestError, DigestInputs, DigestSummarizer,
    DigestSummary, RuleCandidate,
};
use cyberclaw_control_plane::daily_digest_runtime::{RepositoryPersister, StoreDigestCollector};
use cyberclaw_control_plane::execution_service::ExecutionService;
use cyberclaw_control_plane::types::{ExecutionPlan, Resolution};
use cyberclaw_core::execution::ExecutionStatus;
use cyberclaw_core::ids::AgentId;

use common::TestServer;

/// Trivial summarizer — produces a non-empty shape so the coordinator
/// writes an entry through the repository.
struct StaticSummarizer;

#[async_trait::async_trait]
impl DigestSummarizer for StaticSummarizer {
    async fn summarize(
        &self,
        _c: &DailyDigestConfig,
        _i: &DigestInputs,
    ) -> Result<(DigestSummary, Vec<RuleCandidate>), DigestError> {
        Ok((
            DigestSummary {
                facts_md: "## 事实\n- 1 完成".into(),
                problems_md: "## 问题\n_none_".into(),
                learnings_md: "## 学到\n- placeholder".into(),
            },
            vec![RuleCandidate {
                rule: "sprint-9 placeholder rule".into(),
                source_executions: vec![],
            }],
        ))
    }
}

fn plan_for(agent_id: &AgentId) -> ExecutionPlan {
    ExecutionPlan {
        resolution: Resolution {
            agent: agent_id.clone(),
            skills: vec![],
            workflow: None,
            connectors: vec![],
            capabilities: vec![],
            reasons: vec!["L9 digest e2e".into()],
        },
        actions: vec![],
        review_required: false,
        max_fix_loops: cyberclaw_control_plane::default_max_fix_loops(),
        expected_outcomes: vec![],
    }
}

#[tokio::test]
async fn execution_flows_to_digest_endpoint() {
    let server = TestServer::new();

    // Pick an agent id that will not collide across parallel test runs.
    let agent_id =
        AgentId::from_string(format!("digest-e2e-{}", uuid::Uuid::new_v4().simple())).unwrap();

    // --- Seed one execution through the server's ExecutionService.
    let exec_svc: Arc<dyn ExecutionService> = server.state.execution_service.clone();
    let exec_id = exec_svc
        .submit_plan(plan_for(&agent_id))
        .await
        .expect("submit_plan");
    // Running transition sets started_at to now(), which puts it inside the
    // lookback window used below.
    exec_svc
        .update_status(&exec_id, ExecutionStatus::Running)
        .await
        .expect("update to running");
    exec_svc
        .update_status(&exec_id, ExecutionStatus::Completed)
        .await
        .expect("update to completed");

    // --- Run the full 5-stage coordinator.
    let coord = DefaultDailyDigestCoordinator::new(
        Box::new(StoreDigestCollector::new(exec_svc.clone())),
        Box::new(StaticSummarizer),
        Box::new(RepositoryPersister::new(server.state.digest_repo.clone())),
    );
    let cfg = DailyDigestConfig {
        agent_id: agent_id.clone(),
        window_start: Utc::now() - Duration::days(1),
        window_end: Utc::now() + Duration::seconds(60),
        max_rules: 10,
    };
    let outcome = coord.run(cfg).await.expect("coordinator run");
    assert!(
        !outcome.skipped_empty_day,
        "expected a non-empty digest for the seeded execution"
    );
    let entry = outcome.entry.expect("entry must persist");
    assert_eq!(entry.rules.len(), 1);
    assert_eq!(entry.source_executions.len(), 1);

    // --- GET /api/v1/agents/:id/digest?days=30 returns it.
    let uri = format!("/api/v1/agents/{}/digest?days=30", agent_id.as_str());
    let response = server
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(&uri)
                .header("authorization", server.auth_header())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["agent_id"], agent_id.as_str());
    assert!(
        body["count"].as_u64().unwrap() >= 1,
        "expected at least one entry in payload: {}",
        body
    );
    assert!(body["entries"].is_array());

    // Clean up the placeholder fs file so repeated runs stay idempotent.
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    if let Some(home) = home {
        let file = home
            .join(".cyberclaw")
            .join("digests")
            .join(agent_id.as_str())
            .join(format!("{}.json", entry.window_end.format("%Y-%m-%d")));
        let _ = std::fs::remove_file(&file);
        // Best-effort cleanup of the agent directory (ignore ENOTEMPTY etc).
        let _ = std::fs::remove_dir(
            home.join(".cyberclaw")
                .join("digests")
                .join(agent_id.as_str()),
        );
    }
}

#[tokio::test]
async fn digest_endpoint_returns_empty_for_unknown_agent() {
    let server = TestServer::new();
    let agent = format!("no-such-{}", uuid::Uuid::new_v4().simple());
    let response = server
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/agents/{}/digest?days=30", agent))
                .header("authorization", server.auth_header())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["count"].as_u64().unwrap(), 0);
    assert!(body["entries"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn digest_endpoint_rejects_invalid_agent_id() {
    let server = TestServer::new();
    // `..` is rejected by AgentId::from_string.
    let response = server
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/agents/bad..id/digest")
                .header("authorization", server.auth_header())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
