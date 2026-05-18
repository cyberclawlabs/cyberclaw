//! Cron job store and background scheduler.
//!
//! # Components
//!
//! * [`CronJob`] — persisted job definition.
//! * [`CronStore`] — in-memory `Mutex<HashMap>` + file backup at
//!   `~/.cyberclaw/cron.toml` (loaded on `AppState::new`, written on every
//!   mutation).
//! * [`spawn_cron_scheduler`] — background tokio task that ticks every 30 s,
//!   fires overdue jobs and updates `last_run_at` / `next_run_at` /
//!   `last_status`.
//!
//! For `action_type`:
//! * `"echo"` — logs the payload via `tracing::info!` and sets status `"ok"`.
//! * `"chat"` / `"webhook"` — currently also log and set status `"ok"` (v1
//!   stubs; real HTTP dispatch is future work).

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// CronJob
// ---------------------------------------------------------------------------

/// One persisted scheduled job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    /// Standard 6-field cron expression (seconds field required by the `cron`
    /// crate): `"0 */5 * * * *"` = every 5 minutes.
    pub schedule: String,
    /// `"echo"` | `"chat"` | `"webhook"`
    pub action_type: String,
    /// Payload passed to the action handler (message text / URL / …).
    pub action_payload: String,
    pub enabled: bool,
    pub last_run_at: Option<DateTime<Utc>>,
    pub next_run_at: Option<DateTime<Utc>>,
    /// `"ok"` | `"failed"` | `"running"` | `None` (never run yet).
    pub last_status: Option<String>,
}

impl CronJob {
    /// Compute the next fire time for this job's schedule, starting from `after`.
    /// Returns `None` if the expression is invalid or exhausted.
    pub fn compute_next_run(schedule_expr: &str, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        Schedule::from_str(schedule_expr).ok()?.after(&after).next()
    }
}

// ---------------------------------------------------------------------------
// On-disk shape
// ---------------------------------------------------------------------------

/// Top-level wrapper for `~/.cyberclaw/cron.toml`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct CronFile {
    #[serde(default)]
    jobs: Vec<CronJob>,
}

// ---------------------------------------------------------------------------
// CronStore
// ---------------------------------------------------------------------------

/// Thread-safe in-memory store backed by a TOML file on disk.
///
/// All public mutating methods (`add`, `update`, `remove`) write through to
/// disk immediately. Read operations are lock-only.
pub struct CronStore {
    inner: Mutex<HashMap<String, CronJob>>,
    path: PathBuf,
}

impl CronStore {
    /// Create a new store, loading existing jobs from `path` if it exists.
    pub fn load(path: PathBuf) -> Arc<Self> {
        let map = load_from_path(&path);
        Arc::new(Self {
            inner: Mutex::new(map),
            path,
        })
    }

    /// Default path: `$HOME/.cyberclaw/cron.toml`.
    pub fn default_path() -> PathBuf {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".cyberclaw").join("cron.toml"))
            .unwrap_or_else(|| PathBuf::from(".cyberclaw").join("cron.toml"))
    }

    /// Load from the default path.
    pub fn load_default() -> Arc<Self> {
        Self::load(Self::default_path())
    }

    /// Return a snapshot of all jobs sorted by name.
    pub fn list(&self) -> Vec<CronJob> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut jobs: Vec<CronJob> = guard.values().cloned().collect();
        jobs.sort_by(|a, b| a.name.cmp(&b.name));
        jobs
    }

    /// Return a single job by id, or `None`.
    pub fn get(&self, id: &str) -> Option<CronJob> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.get(id).cloned()
    }

    /// Insert a new job. Computes `next_run_at` from the schedule expression.
    /// Returns the newly inserted job.
    pub fn add(&self, job: CronJob) -> CronJob {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(job.id.clone(), job.clone());
        self.persist(&guard);
        job
    }

    /// Update fields of an existing job (by id). Returns the updated job or
    /// `None` if the id is not found.
    pub fn update<F>(&self, id: &str, f: F) -> Option<CronJob>
    where
        F: FnOnce(&mut CronJob),
    {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let job = guard.get_mut(id)?;
        f(job);
        let job = job.clone();
        self.persist(&guard);
        Some(job)
    }

    /// Remove a job by id. Returns `true` if it existed.
    pub fn remove(&self, id: &str) -> bool {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let existed = guard.remove(id).is_some();
        if existed {
            self.persist(&guard);
        }
        existed
    }

    // ------------------------------------------------------------------
    // Persistence helpers
    // ------------------------------------------------------------------

    fn persist(&self, guard: &HashMap<String, CronJob>) {
        if let Err(e) = save_to_path(&self.path, guard) {
            warn!(path = %self.path.display(), error = %e, "cron.toml write failed");
        }
    }
}

// ---------------------------------------------------------------------------
// Disk I/O helpers
// ---------------------------------------------------------------------------

fn load_from_path(path: &std::path::Path) -> HashMap<String, CronJob> {
    if !path.exists() {
        return HashMap::new();
    }
    match std::fs::read_to_string(path) {
        Ok(s) => match toml::from_str::<CronFile>(&s) {
            Ok(file) => file.jobs.into_iter().map(|j| (j.id.clone(), j)).collect(),
            Err(e) => {
                warn!(path = %path.display(), error = %e, "cron.toml parse failed; starting empty");
                HashMap::new()
            }
        },
        Err(e) => {
            warn!(path = %path.display(), error = %e, "cron.toml read failed; starting empty");
            HashMap::new()
        }
    }
}

fn save_to_path(path: &std::path::Path, map: &HashMap<String, CronJob>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create_dir_all: {e}"))?;
    }
    let mut jobs: Vec<CronJob> = map.values().cloned().collect();
    jobs.sort_by(|a, b| a.name.cmp(&b.name));
    let file = CronFile { jobs };
    let toml_str = toml::to_string_pretty(&file).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(path, &toml_str).map_err(|e| format!("write: {e}"))?;
    // chmod 600
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Background scheduler
// ---------------------------------------------------------------------------

/// Spawn the cron scheduler background task.
///
/// Ticks every 30 seconds. For every enabled job whose `next_run_at <= now`,
/// fires the action and updates `last_run_at`, `next_run_at`, `last_status`.
pub fn spawn_cron_scheduler(store: Arc<CronStore>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let tick = std::time::Duration::from_secs(30);
        loop {
            tokio::time::sleep(tick).await;
            let now = Utc::now();
            let jobs = store.list();
            for job in jobs {
                if !job.enabled {
                    continue;
                }
                let should_fire = job.next_run_at.map(|t| t <= now).unwrap_or(false);
                if should_fire {
                    fire_job(&store, &job, now);
                }
            }
        }
    })
}

/// Fire a single job immediately (used both by the scheduler loop and by the
/// `POST /api/v1/cron/:id/run` endpoint).
///
/// Updates `last_run_at`, `next_run_at` (recomputed), and `last_status`.
pub fn fire_job(store: &CronStore, job: &CronJob, now: DateTime<Utc>) {
    let status = execute_action(job);
    let next = CronJob::compute_next_run(&job.schedule, now);
    store.update(&job.id, |j| {
        j.last_run_at = Some(now);
        j.next_run_at = next;
        j.last_status = Some(status.clone());
    });
    info!(
        job_id = %job.id,
        job_name = %job.name,
        action_type = %job.action_type,
        status = %status,
        "cron job fired"
    );
}

fn execute_action(job: &CronJob) -> String {
    match job.action_type.as_str() {
        "echo" => {
            info!(
                job_id = %job.id,
                job_name = %job.name,
                payload = %job.action_payload,
                "cron echo action"
            );
            "ok".to_string()
        }
        "chat" => {
            // v1: log only — real LLM dispatch is future work.
            info!(
                job_id = %job.id,
                job_name = %job.name,
                payload = %job.action_payload,
                "cron chat job fired (dispatch not implemented in v1)"
            );
            "ok".to_string()
        }
        "webhook" => {
            // v1: log only — real HTTP POST is future work.
            info!(
                job_id = %job.id,
                job_name = %job.name,
                payload = %job.action_payload,
                "cron webhook job fired (dispatch not implemented in v1)"
            );
            "ok".to_string()
        }
        other => {
            warn!(
                job_id = %job.id,
                action_type = %other,
                "unknown cron action_type; marking failed"
            );
            "failed".to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// Factory helpers used by the API layer
// ---------------------------------------------------------------------------

/// Normalise a user-supplied cron expression.
///
/// The `cron` 0.12 crate requires exactly 6 fields (seconds + standard 5) or
/// a named shorthand (`@hourly` etc.). Users commonly supply standard 5-field
/// expressions (`"0 * * * *"`), which are rejected by the parser.
///
/// This helper prepends `"0 "` (fire at second 0) when the input looks like
/// a 5-field expression: no leading `@`, and the space-split token count is 5.
pub fn normalize_schedule(schedule: &str) -> String {
    let s = schedule.trim();
    if !s.starts_with('@') && s.split_whitespace().count() == 5 {
        format!("0 {s}")
    } else {
        s.to_string()
    }
}

/// Build a new [`CronJob`] from user-supplied fields. Validates the schedule
/// expression and computes `next_run_at`. Returns `Err` if the schedule is
/// invalid.
///
/// 5-field standard cron expressions (e.g. `"0 * * * *"`) are automatically
/// normalised to 6 fields by prepending a `"0"` seconds field.
pub fn build_new_job(
    name: String,
    schedule: String,
    action_type: String,
    action_payload: String,
    enabled: bool,
) -> Result<CronJob, String> {
    let schedule = normalize_schedule(&schedule);
    let next_run_at = {
        let sched = Schedule::from_str(&schedule)
            .map_err(|e| format!("invalid cron expression '{schedule}': {e}"))?;
        sched.after(&Utc::now()).next()
    };
    Ok(CronJob {
        id: Uuid::new_v4().to_string(),
        name,
        schedule,
        action_type,
        action_payload,
        enabled,
        last_run_at: None,
        next_run_at,
        last_status: None,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_schedule_prepends_seconds_for_5_fields() {
        assert_eq!(normalize_schedule("0 * * * *"), "0 0 * * * *");
        assert_eq!(normalize_schedule("*/5 * * * *"), "0 */5 * * * *");
        assert_eq!(normalize_schedule("0 12 * * 1"), "0 0 12 * * 1");
    }

    #[test]
    fn normalize_schedule_leaves_6_fields_unchanged() {
        assert_eq!(normalize_schedule("0 0 * * * *"), "0 0 * * * *");
        assert_eq!(normalize_schedule("0 */5 * * * *"), "0 */5 * * * *");
    }

    #[test]
    fn normalize_schedule_leaves_named_unchanged() {
        assert_eq!(normalize_schedule("@hourly"), "@hourly");
        assert_eq!(normalize_schedule("@daily"), "@daily");
        assert_eq!(normalize_schedule("@monthly"), "@monthly");
    }

    #[test]
    fn build_new_job_accepts_5_field_expression() {
        let job = build_new_job(
            "qa-5f".into(),
            "0 * * * *".into(),
            "echo".into(),
            "test".into(),
            true,
        )
        .expect("5-field expression should be accepted");
        // The stored schedule must be the normalised 6-field form.
        assert_eq!(job.schedule, "0 0 * * * *");
        assert_eq!(job.name, "qa-5f");
        assert!(job.enabled);
    }

    #[test]
    fn build_new_job_accepts_6_field_expression() {
        build_new_job(
            "t".into(),
            "0 0 * * * *".into(),
            "echo".into(),
            "".into(),
            true,
        )
        .expect("6-field expression should be accepted");
    }

    #[test]
    fn build_new_job_accepts_named_expression() {
        build_new_job("t".into(), "@hourly".into(), "echo".into(), "".into(), true)
            .expect("named expression should be accepted");
    }

    #[test]
    fn build_new_job_rejects_invalid_expression() {
        let err = build_new_job(
            "t".into(),
            "not-a-cron".into(),
            "echo".into(),
            "".into(),
            true,
        );
        assert!(err.is_err());
    }
}
