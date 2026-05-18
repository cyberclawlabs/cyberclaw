//! Persistence layer for Skill evolutionary archives.
//!
//! Stores [`SkillVariant`] records (owned by `cyberclaw-control-plane::skill_archive`)
//! so that evolution history survives across process restarts and sessions.
//!
//! # Design
//!
//! cyberclaw-store sits below cyberclaw-control-plane in the dependency graph, so
//! we define a DTO ([`SkillVariantRecord`]) instead of importing `SkillVariant`
//! directly. control-plane provides the `From<&SkillVariant>` conversions on its
//! side. This mirrors how `ExecutionRecord` / `ArtifactRecord` are structured.
//!
//! # Backends
//!
//! - [`SqliteSkillArchiveRepository`] (feature `sqlite`) — rusqlite-backed persistence
//!   that shares the same `Connection` pattern used by [`crate::sqlite::SqliteStateStore`].
//! - [`InMemorySkillArchiveRepository`] — test/default backend.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;

// ============================================================================
// DTO
// ============================================================================

/// A persistence-layer DTO for a single Skill variant.
///
/// All ID and enum fields are represented as `String` so the store does not
/// need to depend on `cyberclaw-core` ID types or `cyberclaw-control-plane`
/// enums. The caller is responsible for producing / parsing these strings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillVariantRecord {
    /// Variant identifier (stringified `VariantId`).
    pub variant_id: String,
    /// The Skill this variant belongs to (stringified `SkillId`).
    pub skill_id: String,
    /// Parent variant identifier, if any (stringified `VariantId`).
    pub parent_variant_id: Option<String>,
    /// Evaluation score (0.0 - 1.0).
    pub score: f32,
    /// Number of children derived from this variant.
    pub child_count: u32,
    /// Case track tag (stringified `CaseTrack` — e.g. "success" / "failure").
    pub track: String,
    /// Patch artifact identifier (stringified `ArtifactId`), if any.
    pub patch_artifact_id: Option<String>,
}

// ============================================================================
// Repository trait
// ============================================================================

/// Persistence interface for Skill evolutionary archives.
///
/// Implementations MUST:
/// - Preserve insertion order when loading an archive.
/// - Upsert on `save_variant` (duplicate `variant_id` overwrites the previous row).
/// - Return `Ok(None)` from `load_variant` for unknown ids (not an error).
#[async_trait]
pub trait SkillArchiveRepository: Send + Sync {
    /// Upsert a variant. Duplicate `variant_id` overwrites the prior row.
    async fn save_variant(&self, record: &SkillVariantRecord) -> Result<()>;

    /// Load a single variant by its identifier.
    async fn load_variant(&self, variant_id: &str) -> Result<Option<SkillVariantRecord>>;

    /// Load all variants for a given skill in insertion order.
    async fn load_archive(&self, skill_id: &str) -> Result<Vec<SkillVariantRecord>>;

    /// Update the `child_count` of a stored variant.
    ///
    /// Returns [`crate::error::StoreError::NotFound`] when the variant does not exist.
    async fn update_child_count(&self, variant_id: &str, new_count: u32) -> Result<()>;

    /// Delete every variant belonging to `skill_id`. No-op if nothing matches.
    async fn delete_archive(&self, skill_id: &str) -> Result<()>;
}

// ============================================================================
// In-memory implementation (default / tests)
// ============================================================================

/// In-memory [`SkillArchiveRepository`] — preserves insertion order.
///
/// Not intended for production; provided so downstream crates can depend on the
/// trait without enabling the `sqlite` feature.
#[derive(Default)]
pub struct InMemorySkillArchiveRepository {
    inner: tokio::sync::RwLock<Vec<SkillVariantRecord>>,
}

impl InMemorySkillArchiveRepository {
    /// Create an empty in-memory repository.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SkillArchiveRepository for InMemorySkillArchiveRepository {
    async fn save_variant(&self, record: &SkillVariantRecord) -> Result<()> {
        let mut guard = self.inner.write().await;
        if let Some(existing) = guard.iter_mut().find(|r| r.variant_id == record.variant_id) {
            *existing = record.clone();
        } else {
            guard.push(record.clone());
        }
        Ok(())
    }

    async fn load_variant(&self, variant_id: &str) -> Result<Option<SkillVariantRecord>> {
        let guard = self.inner.read().await;
        Ok(guard.iter().find(|r| r.variant_id == variant_id).cloned())
    }

    async fn load_archive(&self, skill_id: &str) -> Result<Vec<SkillVariantRecord>> {
        let guard = self.inner.read().await;
        Ok(guard
            .iter()
            .filter(|r| r.skill_id == skill_id)
            .cloned()
            .collect())
    }

    async fn update_child_count(&self, variant_id: &str, new_count: u32) -> Result<()> {
        let mut guard = self.inner.write().await;
        match guard.iter_mut().find(|r| r.variant_id == variant_id) {
            Some(r) => {
                r.child_count = new_count;
                Ok(())
            }
            None => Err(crate::error::StoreError::NotFound(format!(
                "SkillVariant {} not found",
                variant_id
            ))),
        }
    }

    async fn delete_archive(&self, skill_id: &str) -> Result<()> {
        let mut guard = self.inner.write().await;
        guard.retain(|r| r.skill_id != skill_id);
        Ok(())
    }
}

// ============================================================================
// SQLite implementation (feature `sqlite`)
// ============================================================================

#[cfg(feature = "sqlite")]
pub use sqlite_impl::SqliteSkillArchiveRepository;

#[cfg(feature = "sqlite")]
mod sqlite_impl {
    use super::*;
    use crate::error::StoreError;
    use rusqlite::{params, Connection};
    use std::sync::Mutex;

    /// rusqlite-backed [`SkillArchiveRepository`].
    ///
    /// Matches the pattern used by [`crate::sqlite::SqliteStateStore`]: a single
    /// [`Connection`] guarded by a [`Mutex`], with WAL mode + NORMAL sync.
    pub struct SqliteSkillArchiveRepository {
        conn: Mutex<Connection>,
    }

    impl SqliteSkillArchiveRepository {
        /// Open a repository backed by `path`.
        pub fn new(path: &str) -> Result<Self> {
            let conn = Connection::open(path)
                .map_err(|e| StoreError::ConnectionError(format!("SQLite open: {}", e)))?;
            Self::configure_and_migrate(conn)
        }

        /// Open an in-memory repository (useful for tests).
        pub fn in_memory() -> Result<Self> {
            let conn = Connection::open_in_memory()
                .map_err(|e| StoreError::ConnectionError(format!("SQLite in-memory: {}", e)))?;
            Self::configure_and_migrate(conn)
        }

        fn configure_and_migrate(conn: Connection) -> Result<Self> {
            conn.execute_batch("PRAGMA journal_mode = WAL;")
                .map_err(|e| StoreError::ConnectionError(format!("WAL pragma: {}", e)))?;
            conn.execute_batch("PRAGMA synchronous = NORMAL;")
                .map_err(|e| StoreError::ConnectionError(format!("synchronous pragma: {}", e)))?;
            conn.execute_batch("PRAGMA busy_timeout = 5000;")
                .map_err(|e| StoreError::ConnectionError(format!("busy_timeout pragma: {}", e)))?;

            let store = Self {
                conn: Mutex::new(conn),
            };
            store.run_migrations()?;
            Ok(store)
        }

        fn run_migrations(&self) -> Result<()> {
            let conn = self
                .conn
                .lock()
                .map_err(|e| StoreError::InternalError(format!("mutex poisoned: {}", e)))?;
            let runner = crate::migration::MigrationRunner::new();
            runner.run_sqlite(&conn)?;
            Ok(())
        }

        fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
            self.conn
                .lock()
                .map_err(|e| StoreError::InternalError(format!("mutex poisoned: {}", e)))
        }
    }

    fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<SkillVariantRecord> {
        let child_count: i64 = row.get(4)?;
        Ok(SkillVariantRecord {
            variant_id: row.get(0)?,
            skill_id: row.get(1)?,
            parent_variant_id: row.get(2)?,
            score: row.get::<_, f64>(3)? as f32,
            child_count: child_count.max(0) as u32,
            track: row.get(5)?,
            patch_artifact_id: row.get(6)?,
        })
    }

    #[async_trait]
    impl SkillArchiveRepository for SqliteSkillArchiveRepository {
        async fn save_variant(&self, record: &SkillVariantRecord) -> Result<()> {
            let conn = self.lock()?;
            conn.execute(
                "INSERT INTO skill_variants \
                 (variant_id, skill_id, parent_variant_id, score, child_count, track, patch_artifact_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                 ON CONFLICT(variant_id) DO UPDATE SET \
                     skill_id = excluded.skill_id, \
                     parent_variant_id = excluded.parent_variant_id, \
                     score = excluded.score, \
                     child_count = excluded.child_count, \
                     track = excluded.track, \
                     patch_artifact_id = excluded.patch_artifact_id",
                params![
                    record.variant_id,
                    record.skill_id,
                    record.parent_variant_id,
                    record.score as f64,
                    record.child_count as i64,
                    record.track,
                    record.patch_artifact_id,
                ],
            )?;
            Ok(())
        }

        async fn load_variant(&self, variant_id: &str) -> Result<Option<SkillVariantRecord>> {
            let conn = self.lock()?;
            let mut stmt = conn.prepare(
                "SELECT variant_id, skill_id, parent_variant_id, score, child_count, track, patch_artifact_id \
                 FROM skill_variants WHERE variant_id = ?1",
            )?;
            match stmt.query_row(params![variant_id], row_to_record) {
                Ok(r) => Ok(Some(r)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        }

        async fn load_archive(&self, skill_id: &str) -> Result<Vec<SkillVariantRecord>> {
            let conn = self.lock()?;
            let mut stmt = conn.prepare(
                "SELECT variant_id, skill_id, parent_variant_id, score, child_count, track, patch_artifact_id \
                 FROM skill_variants WHERE skill_id = ?1 ORDER BY rowid ASC",
            )?;
            let rows = stmt.query_map(params![skill_id], row_to_record)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        }

        async fn update_child_count(&self, variant_id: &str, new_count: u32) -> Result<()> {
            let conn = self.lock()?;
            let rows = conn.execute(
                "UPDATE skill_variants SET child_count = ?2 WHERE variant_id = ?1",
                params![variant_id, new_count as i64],
            )?;
            if rows == 0 {
                return Err(StoreError::NotFound(format!(
                    "SkillVariant {} not found",
                    variant_id
                )));
            }
            Ok(())
        }

        async fn delete_archive(&self, skill_id: &str) -> Result<()> {
            let conn = self.lock()?;
            conn.execute(
                "DELETE FROM skill_variants WHERE skill_id = ?1",
                params![skill_id],
            )?;
            Ok(())
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(
        variant_id: &str,
        skill_id: &str,
        parent: Option<&str>,
        score: f32,
    ) -> SkillVariantRecord {
        SkillVariantRecord {
            variant_id: variant_id.to_string(),
            skill_id: skill_id.to_string(),
            parent_variant_id: parent.map(|s| s.to_string()),
            score,
            child_count: 0,
            track: "success".to_string(),
            patch_artifact_id: None,
        }
    }

    // ------------------------------------------------------------------
    // In-memory impl smoke tests
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn inmem_save_and_load_roundtrip() {
        let repo = InMemorySkillArchiveRepository::new();
        let rec = sample("v-1", "s-1", None, 0.5);
        repo.save_variant(&rec).await.unwrap();
        let got = repo.load_variant("v-1").await.unwrap().unwrap();
        assert_eq!(got, rec);
    }

    #[tokio::test]
    async fn inmem_unknown_variant_is_none() {
        let repo = InMemorySkillArchiveRepository::new();
        assert!(repo.load_variant("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn inmem_update_child_count_notfound() {
        let repo = InMemorySkillArchiveRepository::new();
        let res = repo.update_child_count("missing", 1).await;
        assert!(res.is_err());
    }

    // ------------------------------------------------------------------
    // SQLite impl tests (≥8)
    // ------------------------------------------------------------------

    #[cfg(feature = "sqlite")]
    mod sqlite_tests {
        use super::*;

        fn repo() -> SqliteSkillArchiveRepository {
            SqliteSkillArchiveRepository::in_memory().expect("open in-memory sqlite repo")
        }

        #[tokio::test]
        async fn sqlite_save_load_roundtrip_preserves_all_fields() {
            let repo = repo();
            let rec = SkillVariantRecord {
                variant_id: "v-1".to_string(),
                skill_id: "s-a".to_string(),
                parent_variant_id: Some("v-parent".to_string()),
                score: 0.875,
                child_count: 3,
                track: "failure".to_string(),
                patch_artifact_id: Some("artifact-xyz".to_string()),
            };
            repo.save_variant(&rec).await.unwrap();
            let got = repo.load_variant("v-1").await.unwrap().expect("present");
            assert_eq!(got.variant_id, rec.variant_id);
            assert_eq!(got.skill_id, rec.skill_id);
            assert_eq!(got.parent_variant_id, rec.parent_variant_id);
            assert!((got.score - rec.score).abs() < 1e-6);
            assert_eq!(got.child_count, rec.child_count);
            assert_eq!(got.track, rec.track);
            assert_eq!(got.patch_artifact_id, rec.patch_artifact_id);
        }

        #[tokio::test]
        async fn sqlite_load_unknown_returns_none() {
            let repo = repo();
            assert!(repo.load_variant("nope").await.unwrap().is_none());
        }

        #[tokio::test]
        async fn sqlite_save_variant_with_parent_reference() {
            let repo = repo();
            repo.save_variant(&sample("root", "s-1", None, 0.5))
                .await
                .unwrap();
            repo.save_variant(&sample("child", "s-1", Some("root"), 0.7))
                .await
                .unwrap();
            let child = repo.load_variant("child").await.unwrap().unwrap();
            assert_eq!(child.parent_variant_id.as_deref(), Some("root"));
        }

        #[tokio::test]
        async fn sqlite_load_archive_returns_insertion_order() {
            let repo = repo();
            repo.save_variant(&sample("v-1", "s-1", None, 0.1))
                .await
                .unwrap();
            repo.save_variant(&sample("v-2", "s-1", Some("v-1"), 0.2))
                .await
                .unwrap();
            repo.save_variant(&sample("v-3", "s-1", Some("v-2"), 0.3))
                .await
                .unwrap();
            let archive = repo.load_archive("s-1").await.unwrap();
            let ids: Vec<_> = archive.iter().map(|r| r.variant_id.as_str()).collect();
            assert_eq!(ids, vec!["v-1", "v-2", "v-3"]);
        }

        #[tokio::test]
        async fn sqlite_update_child_count_persists() {
            let repo = repo();
            repo.save_variant(&sample("v-1", "s-1", None, 0.5))
                .await
                .unwrap();
            repo.update_child_count("v-1", 7).await.unwrap();
            let got = repo.load_variant("v-1").await.unwrap().unwrap();
            assert_eq!(got.child_count, 7);
        }

        #[tokio::test]
        async fn sqlite_update_child_count_unknown_is_notfound() {
            let repo = repo();
            let err = repo.update_child_count("ghost", 1).await.unwrap_err();
            assert!(matches!(err, crate::error::StoreError::NotFound(_)));
        }

        #[tokio::test]
        async fn sqlite_delete_archive_removes_all_for_skill() {
            let repo = repo();
            repo.save_variant(&sample("v-1", "s-1", None, 0.1))
                .await
                .unwrap();
            repo.save_variant(&sample("v-2", "s-1", None, 0.2))
                .await
                .unwrap();
            repo.save_variant(&sample("v-3", "s-other", None, 0.3))
                .await
                .unwrap();
            repo.delete_archive("s-1").await.unwrap();
            assert!(repo.load_archive("s-1").await.unwrap().is_empty());
            // Other skill untouched
            assert_eq!(repo.load_archive("s-other").await.unwrap().len(), 1);
        }

        #[tokio::test]
        async fn sqlite_archives_are_isolated_per_skill() {
            let repo = repo();
            repo.save_variant(&sample("v-1", "s-a", None, 0.1))
                .await
                .unwrap();
            repo.save_variant(&sample("v-2", "s-b", None, 0.2))
                .await
                .unwrap();
            let a = repo.load_archive("s-a").await.unwrap();
            let b = repo.load_archive("s-b").await.unwrap();
            assert_eq!(a.len(), 1);
            assert_eq!(b.len(), 1);
            assert_eq!(a[0].variant_id, "v-1");
            assert_eq!(b[0].variant_id, "v-2");
        }

        #[tokio::test]
        async fn sqlite_duplicate_variant_id_upserts() {
            let repo = repo();
            repo.save_variant(&sample("v-1", "s-1", None, 0.1))
                .await
                .unwrap();
            let mut updated = sample("v-1", "s-1", None, 0.9);
            updated.child_count = 4;
            updated.track = "failure".to_string();
            repo.save_variant(&updated).await.unwrap();

            let got = repo.load_variant("v-1").await.unwrap().unwrap();
            assert!((got.score - 0.9).abs() < 1e-6);
            assert_eq!(got.child_count, 4);
            assert_eq!(got.track, "failure");

            // Still only one row for this skill
            let archive = repo.load_archive("s-1").await.unwrap();
            assert_eq!(archive.len(), 1);
        }

        #[tokio::test]
        async fn sqlite_delete_empty_archive_is_noop() {
            let repo = repo();
            // Deleting a non-existent skill should not error.
            repo.delete_archive("ghost-skill").await.unwrap();
        }
    }
}
