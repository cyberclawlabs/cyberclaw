//! S20 E3: Background memory cleanup job
//!
//! Spawns a Tokio task that calls `expire_stale` on the leveled memory store
//! at a configurable interval. Stale records (beyond `max_age_days`) are removed;
//! L2Metadata is never expired by the store contract.
//!
//! Environment variables:
//! - `CYBERCLAW_MEMORY_CLEANUP_INTERVAL_SECS` (default: 600)
//! - `CYBERCLAW_MEMORY_RETENTION_DAYS`        (default: 7)

use std::sync::Arc;
use std::time::Duration;

use cyberclaw_store::LeveledMemoryStore;

/// Spawn a background task that periodically calls `expire_stale`.
///
/// - `interval_secs`: how often to run cleanup (minimum enforced to 60s)
/// - `max_age_days`: records older than this many days are removed
pub fn spawn_memory_cleanup(
    memory_store: Arc<dyn LeveledMemoryStore>,
    interval_secs: u64,
    max_age_days: u64,
) {
    // Enforce minimum interval to avoid excessive churn.
    let interval_secs = interval_secs.max(60);

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        // Skip the immediate first tick so cleanup doesn't run right at startup.
        interval.tick().await;
        loop {
            interval.tick().await;
            let max_age = chrono::Duration::days(max_age_days as i64);
            match memory_store.expire_stale(max_age).await {
                Ok(removed) if removed > 0 => {
                    tracing::info!(removed, "memory_cleanup: expired stale records");
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "memory_cleanup: expire_stale failed");
                }
            }
        }
    });
}

/// Load cleanup configuration from environment variables and spawn the job.
///
/// Reads:
/// - `CYBERCLAW_MEMORY_CLEANUP_INTERVAL_SECS` (default: 600)
/// - `CYBERCLAW_MEMORY_RETENTION_DAYS`        (default: 7)
pub fn spawn_memory_cleanup_from_env(memory_store: Arc<dyn LeveledMemoryStore>) {
    let interval_secs: u64 = std::env::var("CYBERCLAW_MEMORY_CLEANUP_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600);

    let max_age_days: u64 = std::env::var("CYBERCLAW_MEMORY_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(7);

    tracing::info!(
        interval_secs,
        max_age_days,
        "memory_cleanup: starting background job"
    );

    spawn_memory_cleanup(memory_store, interval_secs, max_age_days);
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_store::{InMemoryLeveledStore, LeveledMemoryRecord, MemoryLevel};
    use serde_json::json;

    fn make_l0_record(id: &str, session_id: &str, age_hours: i64) -> LeveledMemoryRecord {
        let created_at = chrono::Utc::now() - chrono::Duration::hours(age_hours);
        LeveledMemoryRecord {
            id: id.to_string(),
            session_id: session_id.to_string(),
            agent_id: "agent-test".to_string(),
            level: MemoryLevel::L0Full,
            key: format!("working-{}", id),
            content: json!({"data": id}),
            created_at,
            updated_at: created_at,
            ttl_seconds: MemoryLevel::L0Full.default_ttl_seconds(),
            source_execution_id: Some("exec-test".to_string()),
            embedding: None,
            tags: Vec::new(),
        }
    }

    #[tokio::test]
    async fn test_memory_cleanup_removes_expired() {
        let store = Arc::new(InMemoryLeveledStore::new());

        // Seed a record that is 10 days old (beyond 7-day retention)
        let old = make_l0_record("old-r", "sess-cleanup", 24 * 10);
        store.store_leveled(old).await.unwrap();

        // Seed a fresh record (created just now)
        let fresh = make_l0_record("fresh-r", "sess-cleanup", 0);
        store.store_leveled(fresh).await.unwrap();

        // Call expire_stale directly with 7-day max_age
        let removed = store.expire_stale(chrono::Duration::days(7)).await.unwrap();

        assert_eq!(removed, 1, "only the old record should be removed");

        let remaining = store
            .query_by_level("sess-cleanup", MemoryLevel::L0Full)
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "fresh-r");
    }
}
