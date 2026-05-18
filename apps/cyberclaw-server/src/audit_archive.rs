//! Audit archive background task — RB-11 implementation.
//!
//! Sprint 20 W1. Periodically snapshots the audit DB via
//! `AuditSink::vacuum_into`, verifies the resulting chain, optionally
//! GPG-signs it, and writes both into a configurable directory. A
//! sibling sweeper drops snapshots older than the retention window.
//!
//! # Why this lives in the server, not a CronJob
//!
//! The K8s CronJob template in RB-11 is the *operational* entry point
//! — it runs the same logic out-of-process. This in-process scheduler
//! exists so single-machine deployments (staging, dev, on-prem
//! podman) get audit backups without operator action. Production
//! K8s deployments are encouraged to disable this in-process task
//! (`CYBERCLAW_AUDIT_ARCHIVE_INTERVAL_SECS=0`) and run the CronJob
//! externally so backups survive a server crash.
//!
//! # Configuration env
//!
//!   CYBERCLAW_AUDIT_ARCHIVE_DIR             default "<audit-db-dir>/archive"
//!   CYBERCLAW_AUDIT_ARCHIVE_INTERVAL_SECS   default 3600 (hourly); 0 disables
//!   CYBERCLAW_AUDIT_ARCHIVE_RETAIN_DAYS     default 30
//!   CYBERCLAW_AUDIT_ARCHIVE_GPG_KEY         default unset (no signing)

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tracing::{info, warn};

use crate::audit::AuditSink;

/// Archive task tunables.
#[derive(Debug, Clone)]
pub struct AuditArchiveConfig {
    pub archive_dir: PathBuf,
    pub interval: Duration,
    pub retain_days: i64,
    /// GPG key id for `gpg --detach-sign --local-user <key> ...`.
    /// When `None`, snapshots are emitted unsigned (single-machine dev).
    pub gpg_key: Option<String>,
}

impl AuditArchiveConfig {
    /// Build config from env. The `audit_db_default_dir` is the parent
    /// of the live `audit.db`; archives go to `<dir>/archive` by default.
    pub fn from_env(audit_db_default_dir: &Path) -> Self {
        let archive_dir = std::env::var("CYBERCLAW_AUDIT_ARCHIVE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| audit_db_default_dir.join("archive"));
        let interval = Duration::from_secs(
            std::env::var("CYBERCLAW_AUDIT_ARCHIVE_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(3600),
        );
        let retain_days = std::env::var("CYBERCLAW_AUDIT_ARCHIVE_RETAIN_DAYS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(30);
        let gpg_key = std::env::var("CYBERCLAW_AUDIT_ARCHIVE_GPG_KEY")
            .ok()
            .filter(|s| !s.is_empty());
        Self {
            archive_dir,
            interval,
            retain_days,
            gpg_key,
        }
    }

    pub fn enabled(&self) -> bool {
        self.interval.as_secs() > 0
    }
}

/// Spawn the archive task. Returns immediately; the task lives until
/// the process shuts down.
///
/// When `cfg.interval == 0`, logs the disabled state and returns
/// without spawning anything (production K8s deployments use the
/// CronJob template in RB-11 instead).
pub fn spawn(sink: Arc<AuditSink>, cfg: AuditArchiveConfig) {
    if !cfg.enabled() {
        info!(
            "audit_archive: disabled (CYBERCLAW_AUDIT_ARCHIVE_INTERVAL_SECS=0); use K8s CronJob in RB-11 for production"
        );
        return;
    }

    info!(
        archive_dir = %cfg.archive_dir.display(),
        interval_secs = cfg.interval.as_secs(),
        retain_days = cfg.retain_days,
        gpg = cfg.gpg_key.is_some(),
        "audit_archive: starting background task"
    );

    tokio::spawn(async move {
        if let Err(e) = tokio::fs::create_dir_all(&cfg.archive_dir).await {
            warn!(error = %e, "audit_archive: failed to create archive dir, task will retry");
        }

        loop {
            tokio::time::sleep(cfg.interval).await;
            if let Err(e) = run_once(&sink, &cfg).await {
                warn!(error = %e, "audit_archive: snapshot cycle failed");
            }
            if let Err(e) = sweep_expired(&cfg).await {
                warn!(error = %e, "audit_archive: retention sweep failed");
            }
        }
    });
}

/// Run a single snapshot cycle: VACUUM INTO + verify + optional sign.
/// Public so out-of-process invocations (CLI, CronJob) can reuse it.
pub async fn run_once(sink: &AuditSink, cfg: &AuditArchiveConfig) -> anyhow::Result<PathBuf> {
    let ts = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let dest = cfg.archive_dir.join(format!("audit-{}.db", ts));

    // 1. Snapshot via VACUUM INTO (chain-consistent under WAL).
    sink.vacuum_into(dest.clone()).await?;

    // 2. Verify the snapshot's hash chain matches.
    let report = AuditSink::verify_chain_at(dest.clone()).await?;
    if report.corrupted_at.is_some() {
        anyhow::bail!(
            "audit_archive: snapshot {} fails verify_chain at id={:?}",
            dest.display(),
            report.corrupted_at
        );
    }

    info!(
        path = %dest.display(),
        rows = report.total,
        "audit_archive: snapshot verified"
    );

    // 3. Optional GPG detached signature.
    if let Some(key) = &cfg.gpg_key {
        let sig_path = dest.with_extension("db.asc");
        let key = key.clone();
        let dest_for_gpg = dest.clone();
        let sig_for_gpg = sig_path.clone();
        let gpg_result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let status = std::process::Command::new("gpg")
                .arg("--batch")
                .arg("--yes")
                .arg("--armor")
                .arg("--detach-sign")
                .arg("--local-user")
                .arg(&key)
                .arg("--output")
                .arg(&sig_for_gpg)
                .arg(&dest_for_gpg)
                .status()?;
            if !status.success() {
                anyhow::bail!("gpg detach-sign failed with status {:?}", status.code());
            }
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("audit_archive: gpg join failed: {e}"))?;
        match gpg_result {
            Ok(()) => info!(path = %sig_path.display(), "audit_archive: snapshot signed"),
            Err(e) => {
                warn!(error = %e, "audit_archive: signing failed; snapshot retained unsigned")
            }
        }
    }

    Ok(dest)
}

/// Drop snapshot files older than `cfg.retain_days`. Logs but does NOT
/// fail the parent task on individual delete errors.
pub async fn sweep_expired(cfg: &AuditArchiveConfig) -> anyhow::Result<()> {
    let cutoff = Utc::now() - chrono::Duration::days(cfg.retain_days);
    let mut entries = match tokio::fs::read_dir(&cfg.archive_dir).await {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };

    let mut removed = 0usize;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !name.starts_with("audit-") {
            continue;
        }
        let metadata = match entry.metadata().await {
            Ok(m) => m,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "audit_archive: stat failed during sweep");
                continue;
            }
        };
        let modified = match metadata.modified() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let modified_chrono: chrono::DateTime<Utc> = modified.into();
        if modified_chrono < cutoff {
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {
                    removed += 1;
                    info!(path = %path.display(), "audit_archive: pruned expired snapshot");
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "audit_archive: prune failed");
                }
            }
        }
    }
    if removed > 0 {
        info!(removed, "audit_archive: retention sweep complete");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditEntry, AuditKind, AuditQuery, AuditResult};

    async fn fresh_sink(tmp: &tempfile::TempDir) -> Arc<AuditSink> {
        let sink = AuditSink::new(tmp.path().join("audit.db")).await.unwrap();
        // Seed a few rows so the chain has content.
        for i in 0..5 {
            sink.record(AuditEntry::now(
                "system",
                AuditKind::Mutation,
                format!("test.row.{i}"),
                None,
                serde_json::json!({"i": i}),
                AuditResult::Success,
            ))
            .await;
        }
        Arc::new(sink)
    }

    #[tokio::test]
    async fn run_once_produces_verifiable_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let sink = fresh_sink(&tmp).await;

        let cfg = AuditArchiveConfig {
            archive_dir: tmp.path().join("archive"),
            interval: Duration::from_secs(3600),
            retain_days: 30,
            gpg_key: None,
        };
        tokio::fs::create_dir_all(&cfg.archive_dir).await.unwrap();

        let dest = run_once(&sink, &cfg).await.expect("snapshot ok");
        assert!(dest.exists(), "snapshot file must exist");
        assert!(dest
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("audit-"));

        // Content sanity — can re-open + tail.
        let backup = AuditSink::new(dest.clone()).await.unwrap();
        let rows = backup
            .tail(100, &AuditQuery::default())
            .await
            .expect("tail ok");
        assert_eq!(rows.len(), 5, "snapshot should contain all 5 seeded rows");
    }

    #[tokio::test]
    async fn sweep_removes_files_older_than_cutoff() {
        // Sweep with `retain_days = -1` puts the cutoff in the
        // future, so every file is "expired". This proves the
        // sweep logic identifies + removes correctly without
        // requiring filesystem-level mtime manipulation (which
        // varies across platforms + Rust toolchain versions).
        let tmp = tempfile::tempdir().unwrap();
        let archive_dir = tmp.path().join("archive");
        tokio::fs::create_dir_all(&archive_dir).await.unwrap();

        let target = archive_dir.join("audit-20200101T000000Z.db");
        tokio::fs::write(&target, b"placeholder").await.unwrap();
        let unrelated = archive_dir.join("README.md");
        tokio::fs::write(&unrelated, b"#").await.unwrap();

        let cfg = AuditArchiveConfig {
            archive_dir: archive_dir.clone(),
            interval: Duration::from_secs(3600),
            retain_days: -1, // cutoff in the future → everything expired
            gpg_key: None,
        };

        sweep_expired(&cfg).await.expect("sweep ok");

        assert!(!target.exists(), "audit-* file must be pruned");
        assert!(
            unrelated.exists(),
            "non-audit files must NOT be touched (sweep filters by name prefix)"
        );
    }

    #[tokio::test]
    async fn sweep_keeps_fresh_snapshots() {
        // Inverse of the above: retain_days = 365 days → cutoff
        // in the past → no file old enough to prune.
        let tmp = tempfile::tempdir().unwrap();
        let archive_dir = tmp.path().join("archive");
        tokio::fs::create_dir_all(&archive_dir).await.unwrap();

        let target = archive_dir.join("audit-29990101T000000Z.db");
        tokio::fs::write(&target, b"placeholder").await.unwrap();

        let cfg = AuditArchiveConfig {
            archive_dir: archive_dir.clone(),
            interval: Duration::from_secs(3600),
            retain_days: 365,
            gpg_key: None,
        };

        sweep_expired(&cfg).await.expect("sweep ok");
        assert!(
            target.exists(),
            "fresh snapshot must survive a long retention window"
        );
    }

    #[test]
    fn cfg_disabled_when_interval_zero() {
        let cfg = AuditArchiveConfig {
            archive_dir: PathBuf::from("/tmp"),
            interval: Duration::from_secs(0),
            retain_days: 30,
            gpg_key: None,
        };
        assert!(!cfg.enabled());
    }
}
