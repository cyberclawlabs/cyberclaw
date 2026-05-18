//! Skill provenance + usage telemetry — mirrors Hermes v0.12
//! `tools/skill_provenance.py` + `tools/skill_usage.py`.
//!
//! Two layers:
//!
//! 1. [`WriteOrigin`] — provenance tag attached to every skill write so the
//!    Curator can distinguish background-curator-authored skills from
//!    user-foreground-authored ones. The Curator only consolidates / prunes
//!    skills it created itself.
//!
//! 2. [`SkillUsageStore`] — a sidecar JSON file (default
//!    `<skills_root>/.usage.json`) keyed by skill name. Each tool invocation
//!    bumps `use_count` + `last_used_at`. The Curator reads `last_used_at` to
//!    decide lifecycle transitions (warm / cold / archive candidate).
//!
//! # Why sidecar instead of frontmatter
//!
//! Operational telemetry lives separately from user-authored `SKILL.md`
//! content. Bundled / hub skills don't get conflict pressure when their
//! usage stats are bumped on a deployment that doesn't own the source.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Tag identifying who authored a skill write. Threaded through
/// `SkillHub::install_with_origin` and persisted into the skill's
/// `provenance.origin` frontmatter field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WriteOrigin {
    /// Skill written by the foreground agent on direct user request.
    /// **Curator must not auto-curate these.**
    #[default]
    Foreground,
    /// Skill written by the background self-improvement / Curator review
    /// fork. Eligible for auto-consolidate / auto-prune.
    BackgroundReview,
    /// Seed skill bundled with the platform image (e.g. universal-resilience).
    /// Curator must protect these from mutation.
    SystemSeed,
    /// Skill imported from an external registry / hub.
    HubImport,
}

/// Per-skill usage record. Updated atomically on every invocation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillUsageRecord {
    /// Total number of times this skill was invoked.
    pub use_count: u64,
    /// First-recorded timestamp.
    pub first_used_at: DateTime<Utc>,
    /// Most recent invocation. The Curator's "warm vs cold" decision keys
    /// off this — skills with `last_used_at` older than the cold-cycle
    /// threshold are candidates for archival.
    pub last_used_at: DateTime<Utc>,
    /// Provenance recorded at first install (immutable).
    pub origin: WriteOrigin,
}

impl SkillUsageRecord {
    fn new_with(origin: WriteOrigin, now: DateTime<Utc>) -> Self {
        Self {
            use_count: 0,
            first_used_at: now,
            last_used_at: now,
            origin,
        }
    }

    /// Days since `last_used_at` at the given clock.
    pub fn days_since_last_use(&self, now: DateTime<Utc>) -> i64 {
        now.signed_duration_since(self.last_used_at).num_days()
    }
}

/// Sidecar JSON store for [`SkillUsageRecord`]s. Thread-safe, optional
/// disk persistence. The Curator reads from this; tools mutate it.
#[derive(Debug)]
pub struct SkillUsageStore {
    path: Option<PathBuf>,
    records: RwLock<HashMap<String, SkillUsageRecord>>,
}

impl SkillUsageStore {
    /// In-memory store with no disk persistence (for tests).
    pub fn in_memory() -> Self {
        Self {
            path: None,
            records: RwLock::new(HashMap::new()),
        }
    }

    /// Load from disk if `path` exists, otherwise start empty.
    pub fn load_from(path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        let records = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            serde_json::from_str::<HashMap<String, SkillUsageRecord>>(&raw).unwrap_or_default()
        } else {
            HashMap::new()
        };
        Ok(Self {
            path: Some(path),
            records: RwLock::new(records),
        })
    }

    /// Default location: `<skills_root>/.usage.json`.
    pub fn default_for(skills_root: &Path) -> std::io::Result<Self> {
        Self::load_from(skills_root.join(".usage.json"))
    }

    /// Record a skill *install* event. Idempotent on repeat calls — only
    /// the first install seeds `first_used_at` + `origin`.
    pub fn record_install(&self, skill_name: &str, origin: WriteOrigin) {
        let now = Utc::now();
        let mut g = self.records.write().expect("usage store poisoned");
        g.entry(skill_name.to_string())
            .or_insert_with(|| SkillUsageRecord::new_with(origin, now));
        drop(g);
        self.persist();
    }

    /// Record a skill *invocation* event. Bumps `use_count` and refreshes
    /// `last_used_at`. If unknown, seeds with `Foreground` origin.
    pub fn record_use(&self, skill_name: &str) {
        let now = Utc::now();
        let mut g = self.records.write().expect("usage store poisoned");
        let entry = g
            .entry(skill_name.to_string())
            .or_insert_with(|| SkillUsageRecord::new_with(WriteOrigin::Foreground, now));
        entry.use_count = entry.use_count.saturating_add(1);
        entry.last_used_at = now;
        drop(g);
        self.persist();
    }

    /// Snapshot all known records.
    pub fn snapshot(&self) -> HashMap<String, SkillUsageRecord> {
        self.records.read().expect("usage store poisoned").clone()
    }

    /// Skills authored by `BackgroundReview` whose `last_used_at` is older
    /// than `cold_after_days`. Curator's prune candidates.
    pub fn cold_background_skills(&self, cold_after_days: i64) -> Vec<String> {
        let now = Utc::now();
        self.records
            .read()
            .expect("usage store poisoned")
            .iter()
            .filter(|(_, r)| {
                r.origin == WriteOrigin::BackgroundReview
                    && r.days_since_last_use(now) >= cold_after_days
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Most-used N skills (ranked descending).
    pub fn top_n(&self, n: usize) -> Vec<(String, u64)> {
        let mut rows: Vec<(String, u64)> = self
            .records
            .read()
            .expect("usage store poisoned")
            .iter()
            .map(|(name, r)| (name.clone(), r.use_count))
            .collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row.1));
        rows.truncate(n);
        rows
    }

    /// Test-only: backdate `last_used_at` for a record. Used by the
    /// Curator's tests to simulate aged-out skills without sleeping.
    #[doc(hidden)]
    pub fn _test_set_last_used_at(&self, skill_name: &str, t: DateTime<Utc>) {
        let mut g = self.records.write().expect("usage store poisoned");
        if let Some(r) = g.get_mut(skill_name) {
            r.last_used_at = t;
        }
    }

    /// Forget a skill's record (e.g. after it's deleted).
    pub fn forget(&self, skill_name: &str) {
        let mut g = self.records.write().expect("usage store poisoned");
        g.remove(skill_name);
        drop(g);
        self.persist();
    }

    fn persist(&self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let g = self.records.read().expect("usage store poisoned");
        let body = match serde_json::to_string_pretty(&*g) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "skill usage: serialize failed; skipping persist");
                return;
            }
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(path, body) {
            tracing::warn!(error = %e, path = %path.display(), "skill usage: write failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn record_install_then_use_increments_counter() {
        let store = SkillUsageStore::in_memory();
        store.record_install("alpha", WriteOrigin::BackgroundReview);
        store.record_use("alpha");
        store.record_use("alpha");
        let snap = store.snapshot();
        let r = &snap["alpha"];
        assert_eq!(r.use_count, 2);
        assert_eq!(r.origin, WriteOrigin::BackgroundReview);
        assert!(r.first_used_at <= r.last_used_at);
    }

    #[test]
    fn record_use_on_unknown_skill_seeds_foreground() {
        let store = SkillUsageStore::in_memory();
        store.record_use("ghost");
        let snap = store.snapshot();
        assert_eq!(snap["ghost"].origin, WriteOrigin::Foreground);
        assert_eq!(snap["ghost"].use_count, 1);
    }

    #[test]
    fn cold_background_skills_filters_by_origin_and_age() {
        let store = SkillUsageStore::in_memory();
        store.record_install("recent_bg", WriteOrigin::BackgroundReview);
        store.record_install("user_skill", WriteOrigin::Foreground);
        // Force one record into the past.
        {
            let mut g = store.records.write().unwrap();
            g.get_mut("user_skill").unwrap().last_used_at = Utc::now() - chrono::Duration::days(30);
            // BackgroundReview but recent — should NOT be cold.
        }
        let cold_30 = store.cold_background_skills(7);
        assert!(cold_30.is_empty(), "BackgroundReview was just installed");

        // Now age the BG one too.
        {
            let mut g = store.records.write().unwrap();
            g.get_mut("recent_bg").unwrap().last_used_at = Utc::now() - chrono::Duration::days(30);
        }
        let cold_7 = store.cold_background_skills(7);
        assert_eq!(cold_7, vec!["recent_bg".to_string()]);

        // user_skill is also old but Foreground — must NOT be returned.
        assert!(!cold_7.contains(&"user_skill".to_string()));
    }

    #[test]
    fn top_n_ranks_by_use_count() {
        let store = SkillUsageStore::in_memory();
        store.record_use("a");
        store.record_use("b");
        store.record_use("b");
        store.record_use("c");
        store.record_use("c");
        store.record_use("c");
        let top = store.top_n(2);
        assert_eq!(top, vec![("c".to_string(), 3), ("b".to_string(), 2)]);
    }

    #[test]
    fn persist_round_trip_via_disk() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(".usage.json");
        let store = SkillUsageStore::load_from(&path).unwrap();
        store.record_install("p", WriteOrigin::HubImport);
        store.record_use("p");
        drop(store);

        let store2 = SkillUsageStore::load_from(&path).unwrap();
        let snap = store2.snapshot();
        assert_eq!(snap["p"].use_count, 1);
        assert_eq!(snap["p"].origin, WriteOrigin::HubImport);
    }

    #[test]
    fn forget_removes_record() {
        let store = SkillUsageStore::in_memory();
        store.record_install("x", WriteOrigin::Foreground);
        assert!(store.snapshot().contains_key("x"));
        store.forget("x");
        assert!(!store.snapshot().contains_key("x"));
    }
}
