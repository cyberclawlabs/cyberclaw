#![allow(clippy::duplicate_mod)]
//! E2E tests for Sprint 21 path #2 — POST /api/v1/learning/evolution/run
//!
//! Two flavors:
//!
//! 1. `evolution_run_e2e_with_mock_llm` — runs in default `cargo test`
//!    pipeline. Uses [`MockLlmClient`] which returns a fixed "this is a
//!    mock response" string. The optimizer cannot converge against that
//!    (case scoring expects parseable `{hit:bool}` JSON), so the cycle
//!    terminates as either `MaxIterationsReached` or `Failed`. The test
//!    asserts: 202 → cycle_id minted → spawned task terminates → JSONL
//!    row appears in the redirected evolution log.
//!
//! 2. `evolution_run_real_llm_availability_gate` — `#[ignore]` flag,
//!    same pattern as `e2e_chat_completion_test.rs::test_e2e_chat_008_*`.
//!    Skipped unless explicitly invoked with `--ignored` and `LLM_PROVIDER`
//!    set. Verifies the endpoint behaves identically against a real LLM
//!    backend (the only thing that should change is which outcome the
//!    optimizer reaches; the contract — 202, cycle_id, log row — is
//!    invariant).
//!
//! How to run the real-LLM gate:
//! ```
//! set -a && source apps/cyberclaw-server/.env && set +a && \
//!   cargo test -p cyberclaw-server --test e2e_evolution_test \
//!     evolution_run_real_llm_availability_gate -- --ignored --nocapture
//! ```

#[path = "common/mod.rs"]
mod common;

use std::time::Duration;

use axum::http::StatusCode;
use serde_json::json;
use serial_test::serial;

use cyberclaw_core::ids::UserId;
use cyberclaw_core::users::{OperatorRecord, UsersConfig};

use common::TestServer;

// ---------------------------------------------------------------------------
// $HOME guard — mirrors handoff_review_integration / memory_semantic
// ---------------------------------------------------------------------------

struct RestoreHome {
    original: Option<std::ffi::OsString>,
}

impl RestoreHome {
    fn set(new_home: &std::path::Path) -> Self {
        let original = std::env::var_os("HOME");
        // SAFETY: tests are serialised with `#[serial]`.
        unsafe {
            std::env::set_var("HOME", new_home);
        }
        Self { original }
    }
}

impl Drop for RestoreHome {
    fn drop(&mut self) {
        unsafe {
            match self.original.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

struct RestoreEnv {
    key: &'static str,
    original: Option<std::ffi::OsString>,
}

impl RestoreEnv {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let original = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, original }
    }
}

impl Drop for RestoreEnv {
    fn drop(&mut self) {
        unsafe {
            match self.original.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

/// Seed `users.toml` with one admin operator at `test-user-123`.
///
/// `TestServer::generate_jwt()` hard-codes `sub = "test-user-123"`, so
/// the admin role must be granted to that exact id for `require_admin`
/// to pass.
fn seed_admin_test_user() -> (tempfile::TempDir, RestoreHome) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let restore = RestoreHome::set(tmp.path());

    let uid = UserId::from_string("test-user-123".to_string()).expect("valid user id");
    let record = OperatorRecord {
        user_id: uid,
        display_name: "Admin Tester".to_string(),
        created_at: "2026-05-03T00:00:00Z".to_string(),
        last_login: None,
        role: "admin".to_string(),
        onboarded_at: None,
        intent_auto_route: false,
    };
    let mut cfg = UsersConfig::default();
    cfg.upsert(record);
    let path = UsersConfig::default_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    cfg.save_to_path(&path).expect("save users.toml");

    (tmp, restore)
}

/// Wait for the spawned optimization task to write its terminal cycle
/// row into the redirected log. Mock-LLM callers usually see the row in
/// well under a second; real-LLM callers (availability gate) wrap this
/// in a 120s `tokio::time::timeout`, so the inner poll cap must exceed
/// the outer timeout — otherwise the inner returns `None` before the
/// outer cancellation fires and the test fails for a timing reason
/// rather than a real LLM error. 3000 × 50ms = 150s > 120s outer.
async fn wait_for_log_row(log_path: &std::path::Path, cycle_id: &str) -> Option<String> {
    for _ in 0..3000 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if let Ok(content) = tokio::fs::read_to_string(log_path).await {
            if content.contains(cycle_id) {
                return Some(content);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Test 1 — default pipeline, MockLlmClient
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn evolution_run_e2e_with_mock_llm() {
    let (_admin_tmp, _restore_home) = seed_admin_test_user();

    // Redirect the evolution log to the same temp HOME so it cleans up
    // when the test exits.
    let log_tmp = tempfile::tempdir().expect("log tempdir");
    let log_path = log_tmp.path().join("evolution.log");
    let _restore_log = RestoreEnv::set("CYBERCLAW_EVOLUTION_LOG", &log_path);

    // Seed a real SKILL.md so SkillCreator::optimize gets past the
    // load_initial_skill_text gate and reaches the LLM call. The mock
    // LLM will return non-parseable JSON, driving the optimizer to a
    // terminal Failed/MaxIterationsReached outcome — that's fine; we
    // are testing the HTTP→spawn→log contract, not the optimizer itself.
    let skill_dir = log_tmp.path().join("seed-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: e2e-seed\ndescription: e2e seed skill\n---\nbody\n",
    )
    .unwrap();

    let mut server = TestServer::new();
    let body = json!({
        "skill_id": "sk_e2e_evo",
        "target_skill_path": skill_dir.to_string_lossy(),
        "trigger_dataset": [
            { "query": "draft a python script", "should_trigger": true },
            { "query": "what time is it", "should_trigger": false },
        ],
        "max_iterations": 1,
        "model": "mock-test-model",
    });

    let resp = server.post("/api/v1/learning/evolution/run", body).await;
    assert_eq!(
        resp.status,
        StatusCode::ACCEPTED,
        "expected 202 Accepted, got {} body={:?}",
        resp.status,
        resp.body
    );

    let payload = resp.body.as_ref().expect("response body present");
    let cycle_id = payload
        .get("cycle_id")
        .and_then(|v| v.as_str())
        .expect("cycle_id present")
        .to_string();
    assert!(cycle_id.starts_with("evo_"), "got cycle_id={cycle_id}");
    assert_eq!(
        payload.get("status").and_then(|v| v.as_str()),
        Some("running"),
    );

    let content = wait_for_log_row(&log_path, &cycle_id)
        .await
        .unwrap_or_else(|| {
            panic!(
                "evolution.log never received cycle_id={cycle_id} after 5s. \
                     log_path={}",
                log_path.display()
            )
        });

    let line = content
        .lines()
        .find(|l| l.contains(&cycle_id))
        .expect("cycle line in log");
    let cycle: serde_json::Value = serde_json::from_str(line).expect("cycle line parses as JSON");
    assert_eq!(cycle["id"], cycle_id);
    let outcome = cycle["outcome"].as_str().unwrap_or("");
    assert!(
        matches!(outcome, "converged" | "max_iterations" | "failed"),
        "outcome must be one of the three known variants, got {outcome}"
    );
    assert_eq!(cycle["meta"]["skill_id"], "sk_e2e_evo");
    assert_eq!(cycle["meta"]["model"], "mock-test-model");
}

#[tokio::test]
#[serial]
async fn evolution_run_e2e_rejects_empty_dataset() {
    let (_admin_tmp, _restore_home) = seed_admin_test_user();

    let mut server = TestServer::new();
    let body = json!({
        "skill_id": "sk_e2e_evo",
        "target_skill_path": "/tmp/never-read",
        "trigger_dataset": [],
    });
    let resp = server.post("/api/v1/learning/evolution/run", body).await;
    assert_eq!(
        resp.status,
        StatusCode::BAD_REQUEST,
        "expected 400 InvalidInput, got {} body={:?}",
        resp.status,
        resp.body
    );
}

// ---------------------------------------------------------------------------
// Test 2 — real LLM availability gate (#[ignore], opt-in)
// ---------------------------------------------------------------------------

/// Path #2 real-LLM availability gate. Skipped unless `LLM_PROVIDER` is
/// set to a real provider. Verifies the endpoint contract holds against
/// production-shaped LLM I/O.
#[tokio::test]
#[ignore = "requires external real llm service; run explicitly as availability gate"]
#[serial]
async fn evolution_run_real_llm_availability_gate() {
    let provider = std::env::var("LLM_PROVIDER").unwrap_or_else(|_| "mock".to_string());
    if provider == "mock" || provider.is_empty() {
        println!("evolution_run_real_llm_availability_gate skipped: LLM_PROVIDER is mock/empty");
        return;
    }

    let (_admin_tmp, _restore_home) = seed_admin_test_user();
    let log_tmp = tempfile::tempdir().expect("log tempdir");
    let log_path = log_tmp.path().join("evolution.log");
    let _restore_log = RestoreEnv::set("CYBERCLAW_EVOLUTION_LOG", &log_path);

    let skill_dir = log_tmp.path().join("seed-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: e2e-real-seed\ndescription: real-llm e2e seed\n---\nbody\n",
    )
    .unwrap();

    let mut server = TestServer::new_with_env_llm_config(cyberclaw_server::ServerConfig::default());
    let body = json!({
        "skill_id": "sk_e2e_evo_real",
        "target_skill_path": skill_dir.to_string_lossy(),
        "trigger_dataset": [
            { "query": "draft a python script", "should_trigger": true },
            { "query": "what time is it", "should_trigger": false },
        ],
        "max_iterations": 1,
    });
    let resp = server.post("/api/v1/learning/evolution/run", body).await;
    assert_eq!(
        resp.status,
        StatusCode::ACCEPTED,
        "real-LLM endpoint must still return 202; got {}",
        resp.status
    );
    let payload = resp.body.as_ref().expect("response body present");
    let cycle_id = payload["cycle_id"].as_str().unwrap().to_string();
    let content = tokio::time::timeout(
        Duration::from_secs(120),
        wait_for_log_row(&log_path, &cycle_id),
    )
    .await
    .expect("real LLM cycle did not write log within 120s")
    .expect("log row never appeared");
    println!(
        "evolution_run_real_llm_availability_gate: log content = {}",
        content
    );
}
