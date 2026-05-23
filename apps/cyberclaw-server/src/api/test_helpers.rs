//! Shared test helpers for api module unit tests.
//!
//! Kept `#[cfg(test)]` so it's dropped from release builds. Internal
//! siblings (skills / channels / cluster / settings / events tests) all
//! import [`build_test_state`] to avoid duplicating the ~50-line
//! Control-Plane constructor.

#![cfg(test)]

use std::sync::Arc;

use cyberclaw_connectors::registry::ConnectorRegistry;
use cyberclaw_control_plane::{
    event_bus::InMemoryEventBus, execution_service::InMemoryExecutionService,
    gateway_router::InMemoryGatewayRouter, lease_manager::InMemoryLeaseManager,
    membership_service::InMemoryMembershipService, orchestrator::ControlPlaneOrchestrator,
    placement_engine::InMemoryPlacementEngine, registry::InMemoryRegistry,
    resolver::InMemoryResolver, review_queue::InMemoryReviewQueue,
    task_manager::InMemoryTaskManager,
};
use cyberclaw_core::ids::UserId;
use cyberclaw_governance::engine::DefaultPolicyEngine;
use cyberclaw_llm::prelude::GenericOpenAiClient;
use cyberclaw_llm::LlmClient;
use cyberclaw_observability::events::InMemoryEventRecorder;
use cyberclaw_observability::security_event_store::InMemorySecurityEventStore;

use crate::middleware::auth::generate_jwt;
use crate::state::{AppState, ControlPlaneComponents};

/// Build an [`AppState`] with every dependency wired to an in-memory stub.
///
/// Safe to call from parallel tests — each call constructs an isolated
/// graph (new stores, new plugin registry path, new jwt secret).
pub fn build_test_state() -> Arc<AppState> {
    let llm_client: Arc<dyn LlmClient> =
        Arc::new(GenericOpenAiClient::new(None, "http://localhost:8080").unwrap());
    let connector_registry = Arc::new(ConnectorRegistry::new());
    let policy_engine = Arc::new(DefaultPolicyEngine::default());
    let event_recorder = Arc::new(InMemoryEventRecorder::new());
    let task_manager = Arc::new(InMemoryTaskManager::new());
    let execution_service = Arc::new(InMemoryExecutionService::with_event_recorder(
        event_recorder.clone(),
    ));
    let registry = Arc::new(InMemoryRegistry::new());
    let resolver = Arc::new(InMemoryResolver::new(registry.clone()));
    let review_queue = Arc::new(InMemoryReviewQueue::new(None));
    let gateway = Arc::new(InMemoryGatewayRouter::new());
    let placement_engine = Arc::new(InMemoryPlacementEngine);
    let lease_manager = Arc::new(InMemoryLeaseManager::default());
    let membership_service = Arc::new(InMemoryMembershipService::default());
    let event_bus = Arc::new(InMemoryEventBus::default());
    let security_event_store = Arc::new(InMemorySecurityEventStore::new());
    let plugin_path = std::env::temp_dir().join(format!(
        "cyberclaw-test-plugins-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let plugin_registry = Arc::new(cyberclaw_plugin_runtime::PluginRegistry::new(plugin_path));
    let control_plane = Arc::new(
        ControlPlaneOrchestrator::new(
            gateway,
            resolver,
            registry.clone(),
            review_queue.clone(),
            task_manager.clone(),
            execution_service.clone(),
            placement_engine,
            lease_manager,
            membership_service,
            event_bus,
            policy_engine.clone(),
            plugin_registry,
            security_event_store,
        )
        .with_event_recorder(event_recorder.clone()),
    );
    let cp_components = ControlPlaneComponents {
        control_plane,
        task_manager,
        execution_service,
        review_queue,
        package_registry: registry,
    };
    let mut state = AppState::new(
        llm_client,
        connector_registry,
        policy_engine,
        event_recorder,
        "test-api-jwt-secret".to_string(),
        cp_components,
    );
    // S21 T20 — tests exercise the handoff happy path; opt in regardless of
    // process env (default is false in production).
    state.handoff_feature_enabled = true;
    Arc::new(state)
}

/// Build an [`AppState`] pre-wired with a fresh in-memory ClarifyQueue and
/// ClarifyCoordinator. Suitable for clarify handler unit tests.
///
/// The clarify fields are always present in the base [`build_test_state`] now
/// (they are constructed inside `AppState::new`), so this is a thin alias
/// that exists for readability and future extensibility.
pub fn build_test_state_with_clarify() -> Arc<AppState> {
    build_test_state()
}

/// Issue a JWT signed with the state's secret for the given user id.
pub fn jwt_for(state: &Arc<AppState>, user: &str) -> String {
    let uid = UserId::from_string(user.to_string()).expect("valid user id");
    generate_jwt(&uid, state.jwt_secret.as_bytes(), 3600).expect("jwt")
}

/// RAII guard that points `$HOME` at `new_home` for the test body and
/// restores the original value when dropped. Needed so [`UsersConfig`]
/// sees a temp users.toml instead of the real `~/.cyberclaw/users.toml`.
pub struct RestoreHome {
    original: Option<std::ffi::OsString>,
}

impl RestoreHome {
    pub fn set(new_home: &std::path::Path) -> Self {
        let original = std::env::var_os("HOME");
        // SAFETY: tests tagged `#[serial]` serialize access to `$HOME`.
        unsafe {
            std::env::set_var("HOME", new_home);
        }
        Self { original }
    }
}

impl Drop for RestoreHome {
    fn drop(&mut self) {
        // SAFETY: see `set`.
        unsafe {
            match self.original.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

/// Seed a temp `users.toml` containing a single operator with the given
/// `role`, and point `$HOME` at the tempdir. Returns `(tempdir, restore)` —
/// the caller MUST keep both alive for the duration of the test.
pub fn seed_users_with_role(user_id: &str, role: &str) -> (tempfile::TempDir, RestoreHome) {
    use cyberclaw_core::users::{OperatorRecord, UsersConfig};

    let tmp = tempfile::tempdir().expect("tempdir");
    let restore = RestoreHome::set(tmp.path());
    let uid = UserId::from_string(user_id.to_string()).expect("valid user id");

    let record = OperatorRecord {
        user_id: uid,
        display_name: format!("Operator {}", user_id),
        created_at: "2026-04-19T00:00:00Z".to_string(),
        last_login: None,
        role: role.to_string(),
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
