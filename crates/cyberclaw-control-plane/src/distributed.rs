//! # Distributed Brain & Session Externalization
//!
//! Provides `StatelessBrain` for stateless session handling, `AgenticLoopPool` for
//! pooled loop instances, externalized session state, and `BrainCoordinator` for
//! brain-level session placement and migration.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Semaphore};

use crate::cluster::error::ClusterError;
use crate::cluster::node::SessionAssigner;
use cyberclaw_agent_runtime::agentic_loop::AgenticLoop;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors specific to distributed brain operations.
#[derive(Debug, thiserror::Error)]
pub enum DistributedError {
    #[error("pool exhausted: all agentic loops are in use")]
    PoolExhausted,

    #[error("session not found: {0}")]
    SessionNotFound(String),

    #[error("brain not found: {0}")]
    BrainNotFound(String),

    #[error("migration failed: {0}")]
    MigrationFailed(String),

    #[error("session store error: {0}")]
    StoreError(String),

    #[error("cluster error: {0}")]
    Cluster(#[from] ClusterError),
}

// ---------------------------------------------------------------------------
// AgenticLoopPool
// ---------------------------------------------------------------------------

/// A pool of pre-initialized `AgenticLoop` instances with checkout/checkin semantics.
pub struct AgenticLoopPool {
    /// Available loops ready for checkout.
    available: Mutex<Vec<Box<dyn AgenticLoop>>>,
    /// Maximum pool capacity.
    capacity: usize,
    /// Timeout for waiting on a pool slot.
    checkout_timeout: Duration,
    /// Number of currently checked-out loops.
    checked_out: AtomicUsize,
    /// Semaphore controlling concurrent access without busy-waiting.
    semaphore: Arc<Semaphore>,
}

impl AgenticLoopPool {
    /// Create a new pool with the given loops.
    pub fn new(loops: Vec<Box<dyn AgenticLoop>>, checkout_timeout: Duration) -> Self {
        let capacity = loops.len();
        Self {
            available: Mutex::new(loops),
            capacity,
            checkout_timeout,
            checked_out: AtomicUsize::new(0),
            semaphore: Arc::new(Semaphore::new(capacity)),
        }
    }

    /// Create a pool with a default checkout timeout of 5 seconds.
    pub fn with_default_timeout(loops: Vec<Box<dyn AgenticLoop>>) -> Self {
        Self::new(loops, Duration::from_secs(5))
    }

    /// Attempt to check out a loop from the pool.
    ///
    /// Returns `PoolExhausted` if no loop becomes available within the timeout.
    pub async fn checkout(&self) -> Result<Box<dyn AgenticLoop>, DistributedError> {
        let _permit = tokio::time::timeout(self.checkout_timeout, self.semaphore.acquire())
            .await
            .map_err(|_| DistributedError::PoolExhausted)?
            .map_err(|_| DistributedError::PoolExhausted)?;

        // Permit is intentionally forgotten here; checkin() restores it via add_permits.
        _permit.forget();

        let mut available = self.available.lock().await;
        if let Some(loop_instance) = available.pop() {
            self.checked_out.fetch_add(1, Ordering::SeqCst);
            return Ok(loop_instance);
        }

        // This branch should be unreachable if semaphore and pool stay in sync.
        Err(DistributedError::PoolExhausted)
    }

    /// Return a loop to the pool after use.
    pub async fn checkin(&self, loop_instance: Box<dyn AgenticLoop>) {
        let mut available = self.available.lock().await;
        if available.len() < self.capacity {
            available.push(loop_instance);
        }
        self.checked_out.fetch_sub(1, Ordering::SeqCst);
        // Restore the semaphore permit so the next checkout() can proceed.
        self.semaphore.add_permits(1);
    }

    /// Number of loops currently available in the pool.
    pub async fn available_count(&self) -> usize {
        self.available.lock().await.len()
    }

    /// Number of loops currently checked out.
    pub fn checked_out_count(&self) -> usize {
        self.checked_out.load(Ordering::SeqCst)
    }

    /// Total pool capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

// ---------------------------------------------------------------------------
// SessionStore (externalized session state)
// ---------------------------------------------------------------------------

/// Externalized session state — all session data lives outside the brain node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalizedSession {
    /// Unique session identifier.
    pub session_id: String,
    /// The agent handling this session.
    pub agent_id: String,
    /// Key into the shared state store where session state is persisted.
    pub state_key: String,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// Last time the session was active.
    pub last_active: DateTime<Utc>,
    /// The brain node currently owning this session (if assigned).
    pub assigned_brain: Option<String>,
}

impl ExternalizedSession {
    /// Create a new externalized session.
    pub fn new(session_id: String, agent_id: String) -> Self {
        let now = Utc::now();
        let state_key = format!("session:{}:state", session_id);
        Self {
            session_id,
            agent_id,
            state_key,
            created_at: now,
            last_active: now,
            assigned_brain: None,
        }
    }

    /// Touch the session to update last_active timestamp.
    pub fn touch(&mut self) {
        self.last_active = Utc::now();
    }
}

/// Trait for externalized session storage.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Persist a session.
    async fn save_session(&self, session: &ExternalizedSession) -> Result<(), DistributedError>;

    /// Load a session by ID.
    async fn load_session(
        &self,
        session_id: &str,
    ) -> Result<Option<ExternalizedSession>, DistributedError>;

    /// Delete a session.
    async fn delete_session(&self, session_id: &str) -> Result<(), DistributedError>;

    /// List all sessions assigned to a specific brain node.
    async fn list_sessions_for_node(
        &self,
        brain_id: &str,
    ) -> Result<Vec<ExternalizedSession>, DistributedError>;
}

/// In-memory implementation of `SessionStore` for development and testing.
pub struct InMemorySessionStore {
    sessions: Mutex<HashMap<String, ExternalizedSession>>,
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn save_session(&self, session: &ExternalizedSession) -> Result<(), DistributedError> {
        let mut sessions = self.sessions.lock().await;
        sessions.insert(session.session_id.clone(), session.clone());
        Ok(())
    }

    async fn load_session(
        &self,
        session_id: &str,
    ) -> Result<Option<ExternalizedSession>, DistributedError> {
        let sessions = self.sessions.lock().await;
        Ok(sessions.get(session_id).cloned())
    }

    async fn delete_session(&self, session_id: &str) -> Result<(), DistributedError> {
        let mut sessions = self.sessions.lock().await;
        sessions.remove(session_id);
        Ok(())
    }

    async fn list_sessions_for_node(
        &self,
        brain_id: &str,
    ) -> Result<Vec<ExternalizedSession>, DistributedError> {
        let sessions = self.sessions.lock().await;
        let result = sessions
            .values()
            .filter(|s| s.assigned_brain.as_deref() == Some(brain_id))
            .cloned()
            .collect();
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// SessionRequest / SessionResponse
// ---------------------------------------------------------------------------

/// A request to be handled by a brain for a given session.
#[derive(Debug, Clone)]
pub struct SessionRequest {
    /// The user's input or task description.
    pub input: String,
    /// The agent ID to use for this session.
    pub agent_id: String,
}

/// A response from a brain after handling a session request.
#[derive(Debug, Clone)]
pub struct SessionResponse {
    /// The output produced by the agentic loop.
    pub output: String,
    /// Number of iterations consumed.
    pub iterations: u32,
}

// ---------------------------------------------------------------------------
// StatelessBrain
// ---------------------------------------------------------------------------

/// A stateless brain node that processes sessions without holding local state.
///
/// All session state is externalized through a `SessionStore`. The brain merely
/// borrows an `AgenticLoop` from a pool, executes the session request, and
/// returns the loop when done.
pub struct StatelessBrain {
    /// Identifier for this brain node.
    brain_id: String,
    /// Pool of agentic loops for concurrent execution.
    pool: Arc<AgenticLoopPool>,
    /// Externalized session store.
    session_store: Arc<dyn SessionStore>,
    /// Number of sessions currently being handled.
    active_sessions: AtomicUsize,
}

impl StatelessBrain {
    /// Create a new stateless brain.
    pub fn new(
        brain_id: String,
        pool: Arc<AgenticLoopPool>,
        session_store: Arc<dyn SessionStore>,
    ) -> Self {
        Self {
            brain_id,
            pool,
            session_store,
            active_sessions: AtomicUsize::new(0),
        }
    }

    /// Handle a session request.
    ///
    /// Checks out a loop from the pool, loads or creates the session, processes
    /// the request, persists updated session state, and checks the loop back in.
    pub async fn handle_session(
        &self,
        session_id: &str,
        request: SessionRequest,
    ) -> Result<SessionResponse, DistributedError> {
        // Track active sessions.
        self.active_sessions.fetch_add(1, Ordering::SeqCst);

        // Ensure we decrement on all exit paths.
        let result = self.handle_session_inner(session_id, request).await;

        self.active_sessions.fetch_sub(1, Ordering::SeqCst);

        result
    }

    async fn handle_session_inner(
        &self,
        session_id: &str,
        request: SessionRequest,
    ) -> Result<SessionResponse, DistributedError> {
        // Checkout a loop from the pool.
        let loop_instance = self.pool.checkout().await?;

        // Load or create session.
        let mut session = match self.session_store.load_session(session_id).await? {
            Some(s) => s,
            None => {
                let mut s =
                    ExternalizedSession::new(session_id.to_string(), request.agent_id.clone());
                s.assigned_brain = Some(self.brain_id.clone());
                s
            }
        };

        // Update session metadata.
        session.touch();
        session.assigned_brain = Some(self.brain_id.clone());

        // Persist updated session state.
        self.session_store.save_session(&session).await?;

        // Return the loop to the pool — in a real implementation the loop would
        // be driven here; for now we produce a placeholder response to keep the
        // module compilable without wiring full LLM integration.
        self.pool.checkin(loop_instance).await;

        Ok(SessionResponse {
            output: format!(
                "Processed session {} on brain {}",
                session_id, self.brain_id
            ),
            iterations: 1,
        })
    }

    /// Release a session from this brain.
    pub async fn release_session(&self, session_id: &str) -> Result<(), DistributedError> {
        if let Some(mut session) = self.session_store.load_session(session_id).await? {
            session.assigned_brain = None;
            self.session_store.save_session(&session).await?;
        }
        Ok(())
    }

    /// Current number of active sessions being processed.
    pub fn active_session_count(&self) -> usize {
        self.active_sessions.load(Ordering::SeqCst)
    }

    /// Brain node identifier.
    pub fn brain_id(&self) -> &str {
        &self.brain_id
    }
}

// ---------------------------------------------------------------------------
// BrainCoordinator
// ---------------------------------------------------------------------------

/// Coordinates session-to-brain assignment, migration, and failure recovery.
///
/// Uses a `SessionAssigner` (from the cluster module) for placement decisions
/// and a `SessionStore` for durable session metadata.
pub struct BrainCoordinator {
    /// Session assigner strategy (e.g. least-loaded).
    assigner: Arc<dyn SessionAssigner>,
    /// Externalized session store.
    session_store: Arc<dyn SessionStore>,
    /// Brain-id -> brain mapping for migration.
    brains: Mutex<HashMap<String, Arc<StatelessBrain>>>,
}

impl BrainCoordinator {
    /// Create a new coordinator.
    pub fn new(assigner: Arc<dyn SessionAssigner>, session_store: Arc<dyn SessionStore>) -> Self {
        Self {
            assigner,
            session_store,
            brains: Mutex::new(HashMap::new()),
        }
    }

    /// Register a brain node with the coordinator.
    pub async fn register_brain(&self, brain: Arc<StatelessBrain>) {
        let mut brains = self.brains.lock().await;
        brains.insert(brain.brain_id().to_string(), brain);
    }

    /// Unregister a brain node.
    pub async fn unregister_brain(&self, brain_id: &str) {
        let mut brains = self.brains.lock().await;
        brains.remove(brain_id);
    }

    /// Assign a session to a brain node using the configured assigner.
    ///
    /// Creates or updates the session metadata with the assigned brain.
    pub async fn assign_session(&self, session_id: &str) -> Result<String, DistributedError> {
        let brain_id = self.assigner.assign_session(session_id).await?;

        // Load or create session metadata.
        let mut session = match self.session_store.load_session(session_id).await? {
            Some(s) => s,
            None => ExternalizedSession::new(session_id.to_string(), "default".to_string()),
        };

        session.assigned_brain = Some(brain_id.clone());
        session.touch();
        self.session_store.save_session(&session).await?;

        Ok(brain_id)
    }

    /// Migrate a session from one brain to another.
    ///
    /// The session is released from the source brain and re-assigned to the target.
    pub async fn migrate_session(
        &self,
        session_id: &str,
        from_brain: &str,
        to_brain: &str,
    ) -> Result<(), DistributedError> {
        // Verify session exists and is on the source brain.
        let session = self
            .session_store
            .load_session(session_id)
            .await?
            .ok_or_else(|| DistributedError::SessionNotFound(session_id.to_string()))?;

        if session.assigned_brain.as_deref() != Some(from_brain) {
            return Err(DistributedError::MigrationFailed(format!(
                "session {} is not on brain {}",
                session_id, from_brain
            )));
        }

        // Verify target brain is registered.
        {
            let brains = self.brains.lock().await;
            if !brains.contains_key(to_brain) {
                return Err(DistributedError::BrainNotFound(to_brain.to_string()));
            }
        }

        // Release from source brain.
        {
            let brains = self.brains.lock().await;
            if let Some(source) = brains.get(from_brain) {
                source.release_session(session_id).await?;
            }
        }

        // Update session metadata to target brain.
        let mut updated = session;
        updated.assigned_brain = Some(to_brain.to_string());
        updated.touch();
        self.session_store.save_session(&updated).await?;

        Ok(())
    }

    /// Handle a brain failure by migrating all its sessions to other brains.
    ///
    /// Returns the list of session IDs that were successfully migrated.
    pub async fn handle_brain_failure(
        &self,
        failed_brain_id: &str,
    ) -> Result<Vec<String>, DistributedError> {
        let sessions = self
            .session_store
            .list_sessions_for_node(failed_brain_id)
            .await?;

        let mut migrated = Vec::new();

        for session in &sessions {
            // Release from the failed brain's assigner slot.
            let _ = self.assigner.release_session(&session.session_id).await;

            // Re-assign via the assigner to pick a healthy brain.
            match self.assigner.assign_session(&session.session_id).await {
                Ok(new_brain_id) => {
                    let mut updated = session.clone();
                    updated.assigned_brain = Some(new_brain_id);
                    updated.touch();
                    self.session_store.save_session(&updated).await?;
                    migrated.push(session.session_id.clone());
                }
                Err(_) => {
                    // If no brain is available, clear assignment so it can be retried later.
                    let mut updated = session.clone();
                    updated.assigned_brain = None;
                    self.session_store.save_session(&updated).await?;
                }
            }
        }

        // Unregister the failed brain.
        self.unregister_brain(failed_brain_id).await;

        Ok(migrated)
    }

    /// Get all registered brain IDs.
    pub async fn registered_brains(&self) -> Vec<String> {
        let brains = self.brains.lock().await;
        brains.keys().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::node::{HeartbeatMonitor, LeastLoadedAssigner, NodeLoad};
    use cyberclaw_agent_runtime::agentic_loop::{
        AgenticLoop, IterationResult, LoopConfig, LoopSummary,
    };

    // -- Mock AgenticLoop ---------------------------------------------------

    struct MockLoop {
        id: usize,
    }

    #[async_trait]
    impl AgenticLoop for MockLoop {
        async fn init(&mut self, _config: LoopConfig) -> anyhow::Result<()> {
            Ok(())
        }

        async fn next_iteration(&mut self) -> anyhow::Result<IterationResult> {
            Ok(IterationResult::Done(format!("mock-loop-{}", self.id)))
        }

        async fn finalize(&mut self) -> anyhow::Result<LoopSummary> {
            Ok(LoopSummary::default())
        }
    }

    fn make_mock_loops(count: usize) -> Vec<Box<dyn AgenticLoop>> {
        (0..count)
            .map(|i| Box::new(MockLoop { id: i }) as Box<dyn AgenticLoop>)
            .collect()
    }

    fn sample_load(active: u32) -> NodeLoad {
        NodeLoad {
            cpu_percent: 25.0,
            memory_percent: 40.0,
            active_sessions: active,
            capacity: 100,
        }
    }

    // -- AgenticLoopPool tests ----------------------------------------------

    #[tokio::test]
    async fn test_pool_checkout_and_checkin() {
        let pool = AgenticLoopPool::with_default_timeout(make_mock_loops(3));
        assert_eq!(pool.available_count().await, 3);
        assert_eq!(pool.checked_out_count(), 0);

        let loop1 = pool.checkout().await.unwrap();
        assert_eq!(pool.available_count().await, 2);
        assert_eq!(pool.checked_out_count(), 1);

        let loop2 = pool.checkout().await.unwrap();
        assert_eq!(pool.available_count().await, 1);
        assert_eq!(pool.checked_out_count(), 2);

        pool.checkin(loop1).await;
        assert_eq!(pool.available_count().await, 2);
        assert_eq!(pool.checked_out_count(), 1);

        pool.checkin(loop2).await;
        assert_eq!(pool.available_count().await, 3);
        assert_eq!(pool.checked_out_count(), 0);
    }

    #[tokio::test]
    async fn test_pool_exhaustion() {
        let pool = AgenticLoopPool::new(make_mock_loops(1), Duration::from_millis(50));

        let _loop1 = pool.checkout().await.unwrap();

        // Pool is now empty; checkout should timeout.
        let result = pool.checkout().await;
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(matches!(err, DistributedError::PoolExhausted));
    }

    #[tokio::test]
    async fn test_pool_capacity() {
        let pool = AgenticLoopPool::with_default_timeout(make_mock_loops(5));
        assert_eq!(pool.capacity(), 5);
    }

    // -- InMemorySessionStore tests -----------------------------------------

    #[tokio::test]
    async fn test_session_store_save_and_load() {
        let store = InMemorySessionStore::new();
        let session = ExternalizedSession::new("s1".to_string(), "agent-1".to_string());

        store.save_session(&session).await.unwrap();
        let loaded = store.load_session("s1").await.unwrap();
        assert!(loaded.is_some());

        let loaded = loaded.unwrap();
        assert_eq!(loaded.session_id, "s1");
        assert_eq!(loaded.agent_id, "agent-1");
        assert_eq!(loaded.state_key, "session:s1:state");
    }

    #[tokio::test]
    async fn test_session_store_delete() {
        let store = InMemorySessionStore::new();
        let session = ExternalizedSession::new("s1".to_string(), "agent-1".to_string());

        store.save_session(&session).await.unwrap();
        store.delete_session("s1").await.unwrap();

        let loaded = store.load_session("s1").await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_session_store_load_nonexistent() {
        let store = InMemorySessionStore::new();
        let loaded = store.load_session("nonexistent").await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_session_store_list_for_node() {
        let store = InMemorySessionStore::new();

        let mut s1 = ExternalizedSession::new("s1".to_string(), "agent-1".to_string());
        s1.assigned_brain = Some("brain-a".to_string());

        let mut s2 = ExternalizedSession::new("s2".to_string(), "agent-1".to_string());
        s2.assigned_brain = Some("brain-a".to_string());

        let mut s3 = ExternalizedSession::new("s3".to_string(), "agent-1".to_string());
        s3.assigned_brain = Some("brain-b".to_string());

        store.save_session(&s1).await.unwrap();
        store.save_session(&s2).await.unwrap();
        store.save_session(&s3).await.unwrap();

        let brain_a_sessions = store.list_sessions_for_node("brain-a").await.unwrap();
        assert_eq!(brain_a_sessions.len(), 2);

        let brain_b_sessions = store.list_sessions_for_node("brain-b").await.unwrap();
        assert_eq!(brain_b_sessions.len(), 1);

        let brain_c_sessions = store.list_sessions_for_node("brain-c").await.unwrap();
        assert!(brain_c_sessions.is_empty());
    }

    // -- StatelessBrain tests -----------------------------------------------

    #[tokio::test]
    async fn test_stateless_brain_handle_session() {
        let pool = Arc::new(AgenticLoopPool::with_default_timeout(make_mock_loops(2)));
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let brain = StatelessBrain::new("brain-1".to_string(), pool, store.clone());

        let response = brain
            .handle_session(
                "session-1",
                SessionRequest {
                    input: "hello".to_string(),
                    agent_id: "agent-1".to_string(),
                },
            )
            .await
            .unwrap();

        assert!(response.output.contains("session-1"));
        assert!(response.output.contains("brain-1"));
        assert_eq!(brain.active_session_count(), 0); // completed

        // Session should be persisted in the store.
        let session = store.load_session("session-1").await.unwrap().unwrap();
        assert_eq!(session.assigned_brain.as_deref(), Some("brain-1"));
    }

    #[tokio::test]
    async fn test_stateless_brain_release_session() {
        let pool = Arc::new(AgenticLoopPool::with_default_timeout(make_mock_loops(2)));
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let brain = StatelessBrain::new("brain-1".to_string(), pool, store.clone());

        // Handle first to create session.
        brain
            .handle_session(
                "session-1",
                SessionRequest {
                    input: "hello".to_string(),
                    agent_id: "agent-1".to_string(),
                },
            )
            .await
            .unwrap();

        // Release it.
        brain.release_session("session-1").await.unwrap();

        let session = store.load_session("session-1").await.unwrap().unwrap();
        assert!(session.assigned_brain.is_none());
    }

    #[tokio::test]
    async fn test_stateless_brain_pool_exhaustion() {
        let pool = Arc::new(AgenticLoopPool::new(
            make_mock_loops(1),
            Duration::from_millis(50),
        ));
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let brain = StatelessBrain::new("brain-1".to_string(), pool.clone(), store);

        // Exhaust the pool by checking out the only loop.
        let _held = pool.checkout().await.unwrap();

        let result = brain
            .handle_session(
                "session-1",
                SessionRequest {
                    input: "hello".to_string(),
                    agent_id: "agent-1".to_string(),
                },
            )
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            DistributedError::PoolExhausted
        ));
    }

    // -- BrainCoordinator tests ---------------------------------------------

    async fn make_coordinator_with_brains(
        brain_ids: &[&str],
        pool_size: usize,
    ) -> (
        BrainCoordinator,
        Arc<dyn SessionStore>,
        Arc<HeartbeatMonitor>,
    ) {
        let monitor = Arc::new(HeartbeatMonitor::default_timeout());
        for &id in brain_ids {
            monitor.record_heartbeat(id, sample_load(1));
        }

        let assigner: Arc<dyn SessionAssigner> =
            Arc::new(LeastLoadedAssigner::new(monitor.clone()));
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let coordinator = BrainCoordinator::new(assigner, store.clone());

        for &id in brain_ids {
            let pool = Arc::new(AgenticLoopPool::with_default_timeout(make_mock_loops(
                pool_size,
            )));
            let brain = Arc::new(StatelessBrain::new(id.to_string(), pool, store.clone()));
            coordinator.register_brain(brain).await;
        }

        (coordinator, store, monitor)
    }

    #[tokio::test]
    async fn test_coordinator_assign_session() {
        let (coordinator, store, _monitor) =
            make_coordinator_with_brains(&["brain-a", "brain-b"], 2).await;

        let brain_id = coordinator.assign_session("s1").await.unwrap();
        assert!(!brain_id.is_empty());

        // Session should be stored with the assignment.
        let session = store.load_session("s1").await.unwrap().unwrap();
        assert_eq!(session.assigned_brain, Some(brain_id));
    }

    #[tokio::test]
    async fn test_coordinator_migrate_session() {
        let (coordinator, store, _monitor) =
            make_coordinator_with_brains(&["brain-a", "brain-b"], 2).await;

        // Assign session to brain-a.
        let mut session = ExternalizedSession::new("s1".to_string(), "agent-1".to_string());
        session.assigned_brain = Some("brain-a".to_string());
        store.save_session(&session).await.unwrap();

        // Migrate from brain-a to brain-b.
        coordinator
            .migrate_session("s1", "brain-a", "brain-b")
            .await
            .unwrap();

        let updated = store.load_session("s1").await.unwrap().unwrap();
        assert_eq!(updated.assigned_brain.as_deref(), Some("brain-b"));
    }

    #[tokio::test]
    async fn test_coordinator_migrate_wrong_source() {
        let (coordinator, store, _monitor) =
            make_coordinator_with_brains(&["brain-a", "brain-b"], 2).await;

        let mut session = ExternalizedSession::new("s1".to_string(), "agent-1".to_string());
        session.assigned_brain = Some("brain-a".to_string());
        store.save_session(&session).await.unwrap();

        // Attempt migration from wrong source.
        let result = coordinator
            .migrate_session("s1", "brain-b", "brain-a")
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            DistributedError::MigrationFailed(_)
        ));
    }

    #[tokio::test]
    async fn test_coordinator_migrate_nonexistent_session() {
        let (coordinator, _store, _monitor) =
            make_coordinator_with_brains(&["brain-a", "brain-b"], 2).await;

        let result = coordinator
            .migrate_session("nonexistent", "brain-a", "brain-b")
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            DistributedError::SessionNotFound(_)
        ));
    }

    #[tokio::test]
    async fn test_coordinator_handle_brain_failure() {
        let (coordinator, store, monitor) =
            make_coordinator_with_brains(&["brain-a", "brain-b"], 2).await;

        // Assign two sessions to brain-a.
        let mut s1 = ExternalizedSession::new("s1".to_string(), "agent-1".to_string());
        s1.assigned_brain = Some("brain-a".to_string());
        store.save_session(&s1).await.unwrap();

        let mut s2 = ExternalizedSession::new("s2".to_string(), "agent-1".to_string());
        s2.assigned_brain = Some("brain-a".to_string());
        store.save_session(&s2).await.unwrap();

        // Ensure brain-b has fresh heartbeat for reassignment.
        monitor.record_heartbeat("brain-b", sample_load(0));

        // Handle brain-a failure.
        let migrated = coordinator.handle_brain_failure("brain-a").await.unwrap();

        // Both sessions should be migrated.
        assert_eq!(migrated.len(), 2);

        // brain-a should be unregistered.
        let brains = coordinator.registered_brains().await;
        assert!(!brains.contains(&"brain-a".to_string()));
        assert!(brains.contains(&"brain-b".to_string()));

        // Sessions should be re-assigned to brain-b.
        for sid in &migrated {
            let session = store.load_session(sid).await.unwrap().unwrap();
            assert_eq!(session.assigned_brain.as_deref(), Some("brain-b"));
        }
    }

    #[tokio::test]
    async fn test_coordinator_register_unregister() {
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let monitor = Arc::new(HeartbeatMonitor::default_timeout());
        let assigner: Arc<dyn SessionAssigner> = Arc::new(LeastLoadedAssigner::new(monitor));
        let coordinator = BrainCoordinator::new(assigner, store.clone());

        let pool = Arc::new(AgenticLoopPool::with_default_timeout(make_mock_loops(1)));
        let brain = Arc::new(StatelessBrain::new("brain-x".to_string(), pool, store));

        coordinator.register_brain(brain).await;
        assert!(coordinator
            .registered_brains()
            .await
            .contains(&"brain-x".to_string()));

        coordinator.unregister_brain("brain-x").await;
        assert!(!coordinator
            .registered_brains()
            .await
            .contains(&"brain-x".to_string()));
    }

    #[tokio::test]
    async fn test_externalized_session_state_key() {
        let session = ExternalizedSession::new("abc-123".to_string(), "agent-1".to_string());
        assert_eq!(session.state_key, "session:abc-123:state");
        assert!(session.assigned_brain.is_none());
    }

    #[tokio::test]
    async fn test_externalized_session_touch() {
        let mut session = ExternalizedSession::new("s1".to_string(), "agent-1".to_string());
        let before = session.last_active;
        tokio::time::sleep(Duration::from_millis(10)).await;
        session.touch();
        assert!(session.last_active >= before);
    }
}
