//! SQLite + FTS5 backed skill discovery index.
//!
//! Sprint 21 path #3 — gives agents and operators a query API over the
//! installed skill set. Today the SkillHub holds bundles in a JSON lock
//! file with no full-text search; this index mirrors the metadata into
//! SQLite (separate DB so it doesn't entangle with audit.db) and adds an
//! FTS5 virtual table for natural-language queries like "find skills
//! about decks" or "research methodology".
//!
//! Volume considerations: rebuilds are O(n) over installed skills; today
//! n ≈ 20, future deployments up to ~1000. FTS5 is overkill for n=20 but
//! the right shape for the future. The index is best-effort — failures
//! are logged but never block skill registration.
//!
//! # Why a separate DB file (not audit.db)
//!
//! Audit DB has strict append-only / hash-chain semantics. Skill index
//! is mutable (delete + reindex). Mixing them violates audit.db's
//! integrity contract. The index lives at
//! `~/.cyberclaw/skill-index.db` next to `audit.db`.

use cyberclaw_skill_runtime::compat::SkillCompatRegistry;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::task::spawn_blocking;
use tracing::info;

/// One row of skill metadata as exposed by the search endpoint.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillIndexEntry {
    pub skill_id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub source_request_id: Option<String>,
    pub installed_at: i64,
}

/// Index handle. Holds an `Arc<Mutex<Connection>>` so async wrappers can
/// `spawn_blocking` for SQLite ops without closing-over `&mut self`.
pub struct SkillIndex {
    /// Path the index was opened from. Kept for diagnostics + reopen
    /// on rebuild flows.
    #[allow(dead_code)]
    path: PathBuf,
    conn: Arc<Mutex<Connection>>,
    /// Sprint 44 follow-up: optional shared SkillCompat translator registry.
    /// Mirrors `AppState::skill_compat` so callers reaching the SkillIndex
    /// directly (e.g. admin diagnostics, reindex flows) can resolve a
    /// translator without round-tripping through `AppState`.
    compat_registry: Option<Arc<SkillCompatRegistry>>,
}

impl SkillIndex {
    /// Open or create the index at `path`. Creates schema + FTS5 table
    /// + sync triggers on first use.
    pub async fn new(path: PathBuf) -> anyhow::Result<Self> {
        let path_clone = path.clone();
        let conn = spawn_blocking(move || -> anyhow::Result<Connection> {
            if let Some(parent) = path_clone.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let conn = Connection::open(&path_clone)?;
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS skill_meta (
                    skill_id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    description TEXT NOT NULL,
                    category TEXT NOT NULL DEFAULT 'Other',
                    source_request_id TEXT,
                    installed_at INTEGER NOT NULL DEFAULT 0,
                    indexed_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
                );

                CREATE VIRTUAL TABLE IF NOT EXISTS skill_fts USING fts5(
                    skill_id UNINDEXED,
                    name,
                    description,
                    category,
                    tokenize = 'unicode61 remove_diacritics 2'
                );

                CREATE TRIGGER IF NOT EXISTS skill_meta_ai AFTER INSERT ON skill_meta
                BEGIN
                    INSERT INTO skill_fts(skill_id, name, description, category)
                    VALUES (new.skill_id, new.name, new.description, new.category);
                END;

                CREATE TRIGGER IF NOT EXISTS skill_meta_ad AFTER DELETE ON skill_meta
                BEGIN
                    DELETE FROM skill_fts WHERE skill_id = old.skill_id;
                END;

                CREATE TRIGGER IF NOT EXISTS skill_meta_au AFTER UPDATE ON skill_meta
                BEGIN
                    DELETE FROM skill_fts WHERE skill_id = old.skill_id;
                    INSERT INTO skill_fts(skill_id, name, description, category)
                    VALUES (new.skill_id, new.name, new.description, new.category);
                END;
                ",
            )?;
            Ok(conn)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join failed: {e}"))??;

        info!(path = %path.display(), "skill_index: opened");
        Ok(Self {
            path,
            conn: Arc::new(Mutex::new(conn)),
            compat_registry: None,
        })
    }

    /// Builder: attach a shared [`SkillCompatRegistry`] so callers can
    /// reach it via [`SkillIndex::compat_registry`] without consulting
    /// `AppState`. Symmetric with `AppState::skill_compat`.
    pub fn with_compat_registry(mut self, registry: Arc<SkillCompatRegistry>) -> Self {
        self.compat_registry = Some(registry);
        self
    }

    /// Sprint 44 follow-up: read-only accessor for the optional shared
    /// [`SkillCompatRegistry`]. Returns `None` when the index was opened
    /// without a registry attached.
    pub fn compat_registry(&self) -> Option<Arc<SkillCompatRegistry>> {
        self.compat_registry.clone()
    }

    /// Insert or update one skill's metadata. Triggers keep the FTS5
    /// table in sync.
    pub async fn upsert(&self, entry: SkillIndexEntry) -> anyhow::Result<()> {
        let conn = Arc::clone(&self.conn);
        spawn_blocking(move || -> anyhow::Result<()> {
            let conn = conn.lock().map_err(|e| anyhow::anyhow!("lock poison: {e}"))?;
            conn.execute(
                "INSERT INTO skill_meta (skill_id, name, description, category, source_request_id, installed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(skill_id) DO UPDATE SET
                    name = excluded.name,
                    description = excluded.description,
                    category = excluded.category,
                    source_request_id = excluded.source_request_id,
                    installed_at = excluded.installed_at,
                    indexed_at = strftime('%s', 'now')",
                params![
                    entry.skill_id,
                    entry.name,
                    entry.description,
                    entry.category,
                    entry.source_request_id,
                    entry.installed_at,
                ],
            )?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join failed: {e}"))?
    }

    /// Remove a skill by id.
    pub async fn delete(&self, skill_id: String) -> anyhow::Result<()> {
        let conn = Arc::clone(&self.conn);
        spawn_blocking(move || -> anyhow::Result<()> {
            let conn = conn
                .lock()
                .map_err(|e| anyhow::anyhow!("lock poison: {e}"))?;
            conn.execute(
                "DELETE FROM skill_meta WHERE skill_id = ?1",
                params![skill_id],
            )?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join failed: {e}"))?
    }

    /// Full-text search. Empty query returns the most-recently-indexed
    /// `limit` entries. Otherwise uses FTS5 MATCH ranking.
    pub async fn search(&self, query: String, limit: u32) -> anyhow::Result<Vec<SkillIndexEntry>> {
        let conn = Arc::clone(&self.conn);
        spawn_blocking(move || -> anyhow::Result<Vec<SkillIndexEntry>> {
            let conn = conn.lock().map_err(|e| anyhow::anyhow!("lock poison: {e}"))?;
            let q = query.trim();
            let rows: Vec<SkillIndexEntry> = if q.is_empty() {
                let mut stmt = conn.prepare(
                    "SELECT skill_id, name, description, category, source_request_id, installed_at
                     FROM skill_meta
                     ORDER BY indexed_at DESC
                     LIMIT ?1",
                )?;
                let collected: Vec<SkillIndexEntry> = stmt
                    .query_map(params![limit], |r| {
                        Ok(SkillIndexEntry {
                            skill_id: r.get(0)?,
                            name: r.get(1)?,
                            description: r.get(2)?,
                            category: r.get(3)?,
                            source_request_id: r.get(4)?,
                            installed_at: r.get(5)?,
                        })
                    })?
                    .filter_map(|r| r.ok())
                    .collect();
                collected
            } else {
                // v1.1: token-OR with prefix matching. Previous impl wrapped
                // the whole query in quotes → FTS5 treated it as exact-phrase
                // → "powerpoint ppt presentation" returned 0 even though
                // sk_powerpoint exists. New behavior:
                //   "powerpoint ppt deck" → (powerpoint* OR ppt* OR deck*)
                // tokens are FTS5-sanitized (punctuation stripped) and
                // ≥2-char only (FTS5 errors on single-char prefix). Short
                // queries fall back to the original phrase form.
                let cleaned = q.replace('\'', "''").replace('"', "");
                let tokens: Vec<String> = cleaned
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .filter(|t| t.len() >= 2 && t.len() <= 60)
                    .take(8)
                    .map(|t| format!("{}*", t.to_lowercase()))
                    .collect();
                let fts_query = if tokens.is_empty() {
                    format!("\"{cleaned}\"")
                } else {
                    tokens.join(" OR ")
                };
                let mut stmt = conn.prepare(
                    "SELECT m.skill_id, m.name, m.description, m.category, m.source_request_id, m.installed_at
                     FROM skill_fts f
                     JOIN skill_meta m ON f.skill_id = m.skill_id
                     WHERE skill_fts MATCH ?1
                     ORDER BY rank
                     LIMIT ?2",
                )?;
                let collected: Vec<SkillIndexEntry> = stmt
                    .query_map(params![fts_query, limit], |r| {
                        Ok(SkillIndexEntry {
                            skill_id: r.get(0)?,
                            name: r.get(1)?,
                            description: r.get(2)?,
                            category: r.get(3)?,
                            source_request_id: r.get(4)?,
                            installed_at: r.get(5)?,
                        })
                    })?
                    .filter_map(|r| r.ok())
                    .collect();
                collected
            };
            Ok(rows)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join failed: {e}"))?
    }

    /// Total row count (for diagnostics).
    pub async fn count(&self) -> anyhow::Result<u64> {
        let conn = Arc::clone(&self.conn);
        spawn_blocking(move || -> anyhow::Result<u64> {
            let conn = conn
                .lock()
                .map_err(|e| anyhow::anyhow!("lock poison: {e}"))?;
            let n: i64 = conn.query_row("SELECT COUNT(*) FROM skill_meta", [], |r| r.get(0))?;
            Ok(n as u64)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join failed: {e}"))?
    }

    /// Resolve the default index path: `$CYBERCLAW_SKILL_INDEX_DB`
    /// overrides `$HOME/.cyberclaw/skill-index.db`.
    pub fn default_path() -> PathBuf {
        if let Ok(p) = std::env::var("CYBERCLAW_SKILL_INDEX_DB") {
            return PathBuf::from(p);
        }
        std::env::var_os("HOME")
            .map(|h| Path::new(&h).join(".cyberclaw").join("skill-index.db"))
            .unwrap_or_else(|| PathBuf::from(".cyberclaw").join("skill-index.db"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_entry(id: &str, name: &str, desc: &str) -> SkillIndexEntry {
        SkillIndexEntry {
            skill_id: id.to_string(),
            name: name.to_string(),
            description: desc.to_string(),
            category: "Other".to_string(),
            source_request_id: None,
            installed_at: 0,
        }
    }

    #[tokio::test]
    async fn upsert_and_count() {
        let tmp = tempdir().unwrap();
        let idx = SkillIndex::new(tmp.path().join("idx.db")).await.unwrap();
        idx.upsert(make_entry("sk_a", "alpha", "first skill"))
            .await
            .unwrap();
        idx.upsert(make_entry("sk_b", "beta", "second skill"))
            .await
            .unwrap();
        assert_eq!(idx.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn search_empty_query_returns_recent() {
        let tmp = tempdir().unwrap();
        let idx = SkillIndex::new(tmp.path().join("idx.db")).await.unwrap();
        idx.upsert(make_entry("sk_a", "alpha", "first"))
            .await
            .unwrap();
        idx.upsert(make_entry("sk_b", "beta", "second"))
            .await
            .unwrap();
        let results = idx.search("".to_string(), 10).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn search_matches_description_terms() {
        let tmp = tempdir().unwrap();
        let idx = SkillIndex::new(tmp.path().join("idx.db")).await.unwrap();
        idx.upsert(make_entry(
            "sk_a",
            "deck",
            "Build slide presentations from prompts",
        ))
        .await
        .unwrap();
        idx.upsert(make_entry(
            "sk_b",
            "audit",
            "Verify SHA-256 hash chain integrity",
        ))
        .await
        .unwrap();
        let results = idx.search("slide".to_string(), 10).await.unwrap();
        assert_eq!(results.len(), 1, "should match only the deck skill");
        assert_eq!(results[0].skill_id, "sk_a");
    }

    #[tokio::test]
    async fn upsert_replaces_existing_row() {
        let tmp = tempdir().unwrap();
        let idx = SkillIndex::new(tmp.path().join("idx.db")).await.unwrap();
        idx.upsert(make_entry("sk_a", "alpha", "v1")).await.unwrap();
        idx.upsert(make_entry("sk_a", "alpha", "v2")).await.unwrap();
        assert_eq!(idx.count().await.unwrap(), 1);
        let results = idx.search("v2".to_string(), 10).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn compat_registry_defaults_to_none() {
        let tmp = tempdir().unwrap();
        let idx = SkillIndex::new(tmp.path().join("idx.db")).await.unwrap();
        assert!(
            idx.compat_registry().is_none(),
            "fresh index has no attached registry"
        );
    }

    #[tokio::test]
    async fn with_compat_registry_round_trips_arc() {
        let tmp = tempdir().unwrap();
        let registry = Arc::new(SkillCompatRegistry::with_default());
        let idx = SkillIndex::new(tmp.path().join("idx.db"))
            .await
            .unwrap()
            .with_compat_registry(Arc::clone(&registry));
        let got = idx.compat_registry().expect("registry must be present");
        assert!(
            Arc::ptr_eq(&got, &registry),
            "accessor must return the same Arc that was attached"
        );
    }

    #[tokio::test]
    async fn delete_removes_from_fts() {
        let tmp = tempdir().unwrap();
        let idx = SkillIndex::new(tmp.path().join("idx.db")).await.unwrap();
        idx.upsert(make_entry("sk_a", "alpha", "queryable"))
            .await
            .unwrap();
        assert_eq!(
            idx.search("queryable".to_string(), 10).await.unwrap().len(),
            1
        );
        idx.delete("sk_a".to_string()).await.unwrap();
        assert_eq!(
            idx.search("queryable".to_string(), 10).await.unwrap().len(),
            0
        );
    }
}
