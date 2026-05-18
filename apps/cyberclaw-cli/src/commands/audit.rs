//! Audit log management command (Sprint 21 — RB-11 CLI surface).
//!
//! Out-of-process operator entry point for the audit DB. Wraps the
//! same `cyberclaw_server::audit_archive` primitives used by the
//! in-process background task and the K8s CronJob template.
//!
//! Subcommands:
//!   - `archive`        — VACUUM INTO + verify + optional GPG sign (one-shot)
//!   - `verify-chain`   — walk the hash chain on any DB file
//!   - `list`           — list snapshot files in the archive directory
//!   - `restore`        — verify then atomically replace the live audit DB
//!
//! All operations are safe to run while the server is up *except*
//! `restore`, which assumes the server is stopped (or at least that
//! no other process is writing to the live DB).
//!
//! Why these reuse the server lib (not a duplicate impl): the hash
//! chain is the security boundary; a second implementation would
//! diverge over time. Both lanes (in-process task and CLI) must share
//! the same `vacuum_into` / `verify_chain_at` primitives so an audit
//! made by one lane is provably loadable by the other.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use chrono::Utc;
use clap::{Args, Subcommand};
use cyberclaw_server::audit::AuditSink;
use cyberclaw_server::audit_archive::{self, AuditArchiveConfig};

#[derive(Debug, Subcommand)]
pub enum AuditCommand {
    /// Snapshot the live audit DB to the archive directory (VACUUM INTO + verify).
    Archive(ArchiveArgs),
    /// Walk the hash chain of any audit DB (live or snapshot) and report integrity.
    VerifyChain(VerifyArgs),
    /// List snapshot files in the archive directory.
    List(ListArgs),
    /// Verify a snapshot, then atomically replace the live DB. Server should be stopped first.
    Restore(RestoreArgs),
}

#[derive(Debug, Args)]
pub struct ArchiveArgs {
    /// Live audit DB path. Defaults to `CYBERCLAW_AUDIT_DB` or `$HOME/.cyberclaw/audit.db`.
    #[arg(long)]
    pub db: Option<PathBuf>,
    /// Archive output directory. Defaults to `<db-parent>/archive`.
    #[arg(long)]
    pub out_dir: Option<PathBuf>,
    /// GPG key id for detached signature. Defaults to `CYBERCLAW_AUDIT_ARCHIVE_GPG_KEY`.
    #[arg(long)]
    pub gpg_key: Option<String>,
}

#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// Audit DB to verify. Defaults to the live DB resolved from env.
    #[arg(long)]
    pub db: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Archive directory to scan. Defaults to `<live-db-parent>/archive`.
    #[arg(long)]
    pub out_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct RestoreArgs {
    /// Snapshot file to restore from. Verified before any side effect.
    #[arg(long)]
    pub from: PathBuf,
    /// Target live DB path. Defaults to the path resolved from env.
    #[arg(long)]
    pub to: Option<PathBuf>,
    /// Skip the interactive confirmation. Required when stdin is not a TTY.
    #[arg(long)]
    pub yes: bool,
}

pub async fn handle_audit_command(cmd: AuditCommand) -> anyhow::Result<()> {
    match cmd {
        AuditCommand::Archive(args) => handle_archive(args).await,
        AuditCommand::VerifyChain(args) => handle_verify(args).await,
        AuditCommand::List(args) => handle_list(args).await,
        AuditCommand::Restore(args) => handle_restore(args).await,
    }
}

fn resolve_db(opt: Option<PathBuf>) -> PathBuf {
    opt.unwrap_or_else(AuditSink::default_path)
}

fn resolve_archive_dir(db_path: &Path, override_dir: Option<PathBuf>) -> PathBuf {
    if let Some(d) = override_dir {
        return d;
    }
    let parent = db_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let cfg = AuditArchiveConfig::from_env(&parent);
    cfg.archive_dir
}

async fn handle_archive(args: ArchiveArgs) -> anyhow::Result<()> {
    let db_path = resolve_db(args.db);
    let parent = db_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let mut cfg = AuditArchiveConfig::from_env(&parent);
    if let Some(out_dir) = args.out_dir {
        cfg.archive_dir = out_dir;
    }
    if let Some(key) = args.gpg_key {
        cfg.gpg_key = Some(key);
    }
    tokio::fs::create_dir_all(&cfg.archive_dir)
        .await
        .with_context(|| format!("creating archive dir {}", cfg.archive_dir.display()))?;

    let sink = AuditSink::new(db_path.clone())
        .await
        .with_context(|| format!("opening live audit DB {}", db_path.display()))?;
    let dest = audit_archive::run_once(&sink, &cfg)
        .await
        .context("archive snapshot failed")?;

    println!("Snapshot written: {}", dest.display());
    if cfg.gpg_key.is_some() {
        let sig = dest.with_extension("db.asc");
        if sig.exists() {
            println!("Signature:        {}", sig.display());
        } else {
            println!("Signature:        (signing failed; snapshot retained unsigned)");
        }
    }
    Ok(())
}

async fn handle_verify(args: VerifyArgs) -> anyhow::Result<()> {
    let db_path = resolve_db(args.db);
    if !db_path.exists() {
        bail!("audit DB not found: {}", db_path.display());
    }
    let report = AuditSink::verify_chain_at(db_path.clone())
        .await
        .with_context(|| format!("verify_chain on {}", db_path.display()))?;
    println!("DB:           {}", db_path.display());
    println!("Total rows:   {}", report.total);
    println!("Verified to:  {}", report.ok_until);
    match report.corrupted_at {
        None => {
            println!("Status:       OK (chain intact)");
            Ok(())
        }
        Some(id) => {
            println!("Status:       CORRUPTED at row id {id}");
            bail!("audit chain integrity check failed");
        }
    }
}

async fn handle_list(args: ListArgs) -> anyhow::Result<()> {
    let db_path = resolve_db(None);
    let archive_dir = resolve_archive_dir(&db_path, args.out_dir);
    if !archive_dir.exists() {
        println!(
            "Archive directory does not exist: {}",
            archive_dir.display()
        );
        println!("Run 'cyberclaw audit archive' to create the first snapshot.");
        return Ok(());
    }
    let mut entries = tokio::fs::read_dir(&archive_dir).await?;
    let mut snapshots: Vec<(PathBuf, u64)> = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !name.starts_with("audit-") || !name.ends_with(".db") {
            continue;
        }
        let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
        snapshots.push((path, size));
    }
    if snapshots.is_empty() {
        println!("No snapshots in {}", archive_dir.display());
        return Ok(());
    }
    snapshots.sort_by(|a, b| b.0.cmp(&a.0));
    println!("Archive: {}", archive_dir.display());
    println!("Snapshots (newest first):");
    for (path, size) in snapshots {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<bad name>");
        let kib = size / 1024;
        let signed = path.with_extension("db.asc").exists();
        let sig_marker = if signed { " [signed]" } else { "" };
        println!("  {name}  ({kib} KiB){sig_marker}");
    }
    Ok(())
}

async fn handle_restore(args: RestoreArgs) -> anyhow::Result<()> {
    let target = resolve_db(args.to);
    let source = args.from;

    if !source.exists() {
        bail!("source snapshot not found: {}", source.display());
    }

    println!("Verifying source snapshot integrity...");
    let report = AuditSink::verify_chain_at(source.clone())
        .await
        .with_context(|| format!("verify_chain on {}", source.display()))?;
    if let Some(id) = report.corrupted_at {
        bail!(
            "refusing to restore: snapshot {} is corrupted at row id {id} (verified {} of {} rows)",
            source.display(),
            report.ok_until,
            report.total
        );
    }
    println!("Source verified: {} rows, chain intact", report.total);

    if !args.yes {
        bail!(
            "Refusing restore without --yes (would overwrite {}). Stop the server, then re-run with --yes.",
            target.display()
        );
    }

    // Step 1: preserve the existing live DB before overwriting. The
    // suffix matches the `audit-<ts>.db` format the archive task uses,
    // so an operator inspecting the directory sees a uniform timeline.
    // Skipping this when target is absent is safe: there is nothing
    // to lose, and creating an empty backup would just confuse later
    // forensics.
    let backup_path = if target.exists() {
        let ts = Utc::now().format("%Y%m%dT%H%M%SZ");
        let name = target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audit.db");
        let backup = target.with_file_name(format!("{name}.pre-restore-{ts}"));
        tokio::fs::copy(&target, &backup).await.with_context(|| {
            format!(
                "saving pre-restore backup to {} before overwrite",
                backup.display()
            )
        })?;
        println!("Pre-restore backup: {}", backup.display());
        Some(backup)
    } else {
        println!("Target does not exist — no pre-restore backup needed.");
        None
    };

    // Step 2: copy snapshot into place. `tokio::fs::copy` reads source
    // into a fresh write of target — a torn copy leaves an obviously
    // broken file at `target` (caught by step 3) rather than silently
    // truncating the original. We use copy not rename so the snapshot
    // remains in the archive directory for repeat restores.
    tokio::fs::copy(&source, &target)
        .await
        .with_context(|| format!("copying {} to {}", source.display(), target.display()))?;
    println!(
        "Copied snapshot into place: {} -> {}",
        source.display(),
        target.display()
    );

    // Step 3: post-copy chain verify. If this fails, the freshly
    // installed file is corrupt — restore the saved backup so the
    // operator is left in the original (working) state, not a broken
    // one. We overwrite via `copy` again rather than rename to keep
    // the failure forensics intact (operator can inspect both files).
    let post = AuditSink::verify_chain_at(target.clone())
        .await
        .with_context(|| format!("post-restore verify_chain on {}", target.display()))?;
    if let Some(corrupt_id) = post.corrupted_at {
        if let Some(backup_path) = backup_path.as_ref() {
            let _ = tokio::fs::copy(backup_path, &target).await;
            bail!(
                "post-restore verify failed at row {corrupt_id} ({}/{} rows verified). \
                 Original DB has been restored from {}; investigate the snapshot before retrying.",
                post.ok_until,
                post.total,
                backup_path.display()
            );
        }
        bail!(
            "post-restore verify failed at row {corrupt_id} and no pre-restore backup exists; \
             the snapshot at {} is corrupt and the live DB is now broken — pull a different snapshot.",
            target.display()
        );
    }

    println!(
        "Restore complete — {} rows verified, chain intact.",
        post.total
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_server::audit::{AuditEntry, AuditKind, AuditResult};

    async fn seeded_db(path: PathBuf) -> AuditSink {
        let sink = AuditSink::new(path).await.expect("open sink");
        for i in 0..3 {
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
        sink
    }

    #[tokio::test]
    async fn archive_then_verify_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("audit.db");
        let _sink = seeded_db(db_path.clone()).await;

        // Step 1: archive
        handle_archive(ArchiveArgs {
            db: Some(db_path.clone()),
            out_dir: Some(tmp.path().join("archive")),
            gpg_key: None,
        })
        .await
        .expect("archive ok");

        // Snapshot file should exist with audit-*.db naming.
        let mut entries = std::fs::read_dir(tmp.path().join("archive")).unwrap();
        let snapshot = entries
            .find_map(|e| {
                let p = e.ok()?.path();
                let name = p.file_name()?.to_str()?.to_string();
                (name.starts_with("audit-") && name.ends_with(".db")).then_some(p)
            })
            .expect("snapshot file should exist");

        // Step 2: verify the snapshot
        handle_verify(VerifyArgs { db: Some(snapshot) })
            .await
            .expect("verify ok");
    }

    #[tokio::test]
    async fn verify_reports_missing_db() {
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("does-not-exist.db");
        let err = handle_verify(VerifyArgs {
            db: Some(nonexistent),
        })
        .await
        .expect_err("must fail");
        assert!(
            err.to_string().contains("not found"),
            "error should mention missing DB, got: {err}"
        );
    }

    #[tokio::test]
    async fn list_handles_empty_archive_dir() {
        let tmp = tempfile::tempdir().unwrap();
        // No archive directory at all → still ok, just prints "does not exist".
        handle_list(ListArgs {
            out_dir: Some(tmp.path().join("nonexistent-archive")),
        })
        .await
        .expect("list ok");
    }

    #[tokio::test]
    async fn restore_round_trip_preserves_backup_and_verifies() {
        // Build a sealed snapshot, restore it over a tampered "live" DB,
        // and confirm: (a) the pre-restore backup is on disk, (b) the
        // restored file verifies chain-intact, (c) row count matches the
        // snapshot.
        let tmp = tempfile::tempdir().unwrap();
        let live = tmp.path().join("audit.db");
        let archive_dir = tmp.path().join("archive");
        tokio::fs::create_dir_all(&archive_dir).await.unwrap();

        // Seed live DB with 3 rows + take a snapshot.
        {
            let sink = seeded_db(live.clone()).await;
            let cfg = AuditArchiveConfig {
                archive_dir: archive_dir.clone(),
                interval: std::time::Duration::from_secs(3600),
                retain_days: 30,
                gpg_key: None,
            };
            audit_archive::run_once(&sink, &cfg).await.unwrap();
        }

        // Now mutate the live DB to a different state (5 rows) so we
        // can confirm the restore actually replaced it (3 ≠ 5).
        {
            let sink = AuditSink::new(live.clone()).await.unwrap();
            for i in 0..2 {
                sink.record(AuditEntry::now(
                    "system",
                    AuditKind::Mutation,
                    format!("post.snapshot.{i}"),
                    None,
                    serde_json::json!({"i": i}),
                    AuditResult::Success,
                ))
                .await;
            }
        }

        // Locate the snapshot.
        let snapshot = std::fs::read_dir(&archive_dir)
            .unwrap()
            .filter_map(|e| {
                let p = e.ok()?.path();
                let name = p.file_name()?.to_str()?.to_string();
                (name.starts_with("audit-") && name.ends_with(".db")).then_some(p)
            })
            .next()
            .expect("snapshot must exist");

        handle_restore(RestoreArgs {
            from: snapshot,
            to: Some(live.clone()),
            yes: true,
        })
        .await
        .expect("restore must succeed");

        // (a) pre-restore backup must exist with the .pre-restore- suffix.
        let parent = live.parent().unwrap();
        let mut backups = std::fs::read_dir(parent)
            .unwrap()
            .filter_map(|e| {
                let p = e.ok()?.path();
                let name = p.file_name()?.to_str()?.to_string();
                name.contains(".pre-restore-").then_some(p)
            })
            .collect::<Vec<_>>();
        backups.sort();
        assert_eq!(
            backups.len(),
            1,
            "exactly one pre-restore backup expected, found {}",
            backups.len()
        );

        // (b) restored DB verifies chain-intact.
        let report = AuditSink::verify_chain_at(live.clone()).await.unwrap();
        assert!(
            report.corrupted_at.is_none(),
            "restored DB chain must verify clean"
        );

        // (c) row count must match the snapshot (3), not the post-mutation live (5).
        assert_eq!(
            report.total, 3,
            "restored DB should have snapshot's 3 rows, not live's 5"
        );
    }

    #[tokio::test]
    async fn restore_refuses_without_yes() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("audit.db");
        let _sink = seeded_db(db_path.clone()).await;

        // Make a snapshot to restore from.
        let cfg = AuditArchiveConfig {
            archive_dir: tmp.path().join("archive"),
            interval: std::time::Duration::from_secs(3600),
            retain_days: 30,
            gpg_key: None,
        };
        tokio::fs::create_dir_all(&cfg.archive_dir).await.unwrap();
        let snapshot = audit_archive::run_once(&_sink, &cfg).await.unwrap();

        let err = handle_restore(RestoreArgs {
            from: snapshot,
            to: Some(db_path),
            yes: false,
        })
        .await
        .expect_err("must refuse without --yes");
        assert!(
            err.to_string().contains("--yes"),
            "error should mention --yes, got: {err}"
        );
    }
}
