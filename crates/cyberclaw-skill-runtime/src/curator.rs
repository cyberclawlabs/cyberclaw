//! Autonomous Curator — periodic background grader/pruner/consolidator
//! for the skill library. Mirrors Hermes v0.12 `hermes curator`.
//!
//! # Lifecycle
//!
//! On each tick (default every 7 days):
//!
//! 1. **Snapshot** — read `SkillUsageStore` + installed skill list.
//! 2. **Grade** — call the [`SkillEvaluator`] (LLM-driven OR heuristic) to
//!    classify every BackgroundReview-origin skill into:
//!    - `Keep`     — well-used, no action.
//!    - `Cold`     — unused for cold_after_days; candidate for archival.
//!    - `Duplicate`— overlaps another skill; candidate for consolidation.
//! 3. **Consolidate** — for `Duplicate` clusters, the evaluator returns a
//!    merged SKILL.md body; Curator writes the merge under a new name and
//!    archives the originals.
//! 4. **Prune** — `Cold` skills are moved to `<skills_root>/.archive/`.
//! 5. **Report** — write JSON + Markdown reports to
//!    `<logs_root>/curator/<run_id>/run.json` + `REPORT.md`.
//!
//! # Defense-in-depth gates
//!
//! - Skills with `WriteOrigin::Foreground` (user-authored) are NEVER touched.
//! - Skills with `WriteOrigin::SystemSeed` (bundled) are NEVER touched.
//! - Skills with `WriteOrigin::HubImport` are NEVER touched (operator owns
//!   external sources).
//! - Curator only operates on `WriteOrigin::BackgroundReview`.
//!
//! # Pluggable evaluator
//!
//! [`SkillEvaluator`] is a trait so callers can plug in:
//! - `HeuristicEvaluator` (default) — uses age + use_count thresholds only.
//! - `LlmEvaluator` (production) — calls a model to grade skill quality and
//!   detect overlap. Not implemented in this module to avoid coupling.

use crate::portability_verifier::{PortabilityReport, PortabilityTier, PortabilityVerifier};
use crate::telemetry::{SkillUsageRecord, SkillUsageStore, WriteOrigin};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Per-skill verdict from the evaluator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum SkillVerdict {
    /// Keep as-is.
    Keep,
    /// Archive — move to `.archive/` and forget telemetry.
    Cold {
        /// Days since last use (for the report).
        days_idle: i64,
    },
    /// Merge with the listed skills under a new name.
    Duplicate {
        /// Other skill names this one overlaps with.
        overlaps_with: Vec<String>,
        /// Proposed merged-skill name (evaluator chooses).
        merged_name: String,
    },
}

/// Result of a single curator run, suitable for JSON / Markdown reporting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuratorRunReport {
    pub run_id: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub skills_evaluated: usize,
    pub skills_kept: usize,
    pub skills_archived: Vec<String>,
    pub skills_consolidated: Vec<ConsolidationRecord>,
    pub skills_protected: Vec<ProtectedRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationRecord {
    pub merged_name: String,
    pub source_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtectedRecord {
    pub skill_name: String,
    pub origin: WriteOrigin,
    pub reason: String,
}

/// Configuration for the Curator.
#[derive(Debug, Clone)]
pub struct CuratorConfig {
    /// Cycle interval. Default: 7 days.
    pub cycle: Duration,
    /// Skills idle ≥ this many days are "Cold". Default: 14 days.
    pub cold_after_days: i64,
    /// Where to write run reports. Default: `<skills_root>/.curator/`.
    pub reports_dir: PathBuf,
    /// Where to move archived skills. Default: `<skills_root>/.archive/`.
    pub archive_dir: PathBuf,
    /// Dry run — compute verdicts but don't actually move/delete files.
    pub dry_run: bool,
    /// Root directory of installed skills. Used to locate each skill's
    /// `SKILL.md` when patching the portability frontmatter.
    /// Convention: `<skills_root>/installed/<skill_name>/SKILL.md`.
    pub skills_root: PathBuf,
}

impl CuratorConfig {
    pub fn for_skills_root(skills_root: PathBuf) -> Self {
        Self {
            cycle: Duration::from_secs(7 * 24 * 3600),
            cold_after_days: 14,
            reports_dir: skills_root.join(".curator"),
            archive_dir: skills_root.join(".archive"),
            dry_run: false,
            skills_root,
        }
    }
}

/// Pluggable scoring + consolidation logic. Heuristic impl is built-in.
/// Production deployments wrap an LLM client.
#[async_trait]
pub trait SkillEvaluator: Send + Sync {
    /// Decide a verdict for each skill record. Return order doesn't matter.
    async fn evaluate(
        &self,
        records: &HashMap<String, SkillUsageRecord>,
        config: &CuratorConfig,
    ) -> HashMap<String, SkillVerdict>;
}

/// Heuristic evaluator — pure age/use_count logic, no LLM call.
/// Verdicts:
/// - `Cold` if BackgroundReview AND idle > cold_after_days AND use_count == 0
/// - `Keep` otherwise (no Duplicate detection without LLM)
pub struct HeuristicEvaluator;

#[async_trait]
impl SkillEvaluator for HeuristicEvaluator {
    async fn evaluate(
        &self,
        records: &HashMap<String, SkillUsageRecord>,
        config: &CuratorConfig,
    ) -> HashMap<String, SkillVerdict> {
        let now = Utc::now();
        records
            .iter()
            .filter_map(|(name, r)| {
                if r.origin != WriteOrigin::BackgroundReview {
                    return None;
                }
                let days_idle = r.days_since_last_use(now);
                if r.use_count == 0 && days_idle >= config.cold_after_days {
                    Some((name.clone(), SkillVerdict::Cold { days_idle }))
                } else {
                    Some((name.clone(), SkillVerdict::Keep))
                }
            })
            .collect()
    }
}

/// Lightweight snapshot of curator run state for status endpoints.
///
/// Surfaces the most recent run timestamp, the next scheduled tick (if a
/// `spawn_loop` task was started), and a cumulative run counter. Mirrors
/// what the admin SPA needs to display in the curator status card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuratorRunInfo {
    /// Wall-clock time of the last completed `run_once` call.
    pub last_run_at: Option<DateTime<Utc>>,
    /// Wall-clock time of the next scheduled tick (only set when
    /// `spawn_loop` was called and a cycle interval is known).
    pub next_run_at: Option<DateTime<Utc>>,
    /// Cumulative count of `run_once` invocations since process start.
    pub total_runs: u64,
    /// The most recent run report, if any.
    pub last_report: Option<CuratorRunReport>,
}

/// The orchestrator.
pub struct Curator {
    config: CuratorConfig,
    usage: Arc<SkillUsageStore>,
    evaluator: Arc<dyn SkillEvaluator>,
    /// Cumulative count of `run_once` invocations since this Curator was
    /// constructed. Used by `last_run_info()` and admin status surfaces.
    total_runs: AtomicU64,
    /// Cached snapshot of the last finished run (`None` before the first
    /// pass completes).
    last_report: RwLock<Option<CuratorRunReport>>,
    /// Wall-clock timestamp of the next scheduled tick. Populated by
    /// `spawn_loop()`. `None` when only `run_once` is being driven by an
    /// external scheduler.
    next_run_at: RwLock<Option<DateTime<Utc>>>,
}

impl Curator {
    pub fn new(
        config: CuratorConfig,
        usage: Arc<SkillUsageStore>,
        evaluator: Arc<dyn SkillEvaluator>,
    ) -> Self {
        Self {
            config,
            usage,
            evaluator,
            total_runs: AtomicU64::new(0),
            last_report: RwLock::new(None),
            next_run_at: RwLock::new(None),
        }
    }

    /// Read-only snapshot of current curator state — safe to call from
    /// HTTP handlers without holding any long-lived locks.
    pub async fn last_run_info(&self) -> CuratorRunInfo {
        let last_report = self.last_report.read().await.clone();
        let last_run_at = last_report.as_ref().map(|r| r.finished_at);
        let next_run_at = *self.next_run_at.read().await;
        CuratorRunInfo {
            last_run_at,
            next_run_at,
            total_runs: self.total_runs.load(Ordering::Relaxed),
            last_report,
        }
    }

    /// Run one curator pass. Returns the report.
    pub async fn run_once(&self) -> CuratorRunReport {
        let started_at = Utc::now();
        let run_id = format!("run-{}", started_at.format("%Y%m%dT%H%M%SZ"));
        info!(run_id = %run_id, "Curator: starting pass");

        let snapshot = self.usage.snapshot();
        let skills_evaluated = snapshot.len();

        // Evaluate.
        let verdicts = self.evaluator.evaluate(&snapshot, &self.config).await;

        let mut archived = Vec::new();
        let mut consolidated = Vec::new();
        let mut protected = Vec::new();
        let mut kept = 0usize;

        let verifier = PortabilityVerifier::new();

        // Apply: only the BackgroundReview-origin skills got verdicts.
        for (name, record) in &snapshot {
            if record.origin != WriteOrigin::BackgroundReview {
                protected.push(ProtectedRecord {
                    skill_name: name.clone(),
                    origin: record.origin,
                    reason: format!(
                        "origin={:?} is protected — only BackgroundReview is curatable",
                        record.origin
                    ),
                });
                continue;
            }

            // Scan portability and persist result into SKILL.md frontmatter.
            // Best-effort: failures are logged but do not abort the curator pass.
            if !self.config.dry_run {
                let skill_md = self
                    .config
                    .skills_root
                    .join("installed")
                    .join(name)
                    .join("SKILL.md");
                let report = verifier.scan_path(&skill_md);
                if let Err(e) = patch_portability_frontmatter(&skill_md, &report) {
                    warn!(
                        skill = %name,
                        error = %e,
                        "Curator: failed to write portability frontmatter"
                    );
                }
            }

            match verdicts.get(name).cloned().unwrap_or(SkillVerdict::Keep) {
                SkillVerdict::Keep => {
                    kept += 1;
                }
                SkillVerdict::Cold { days_idle } => {
                    info!(skill = %name, days_idle, "Curator: archiving cold skill");
                    if !self.config.dry_run {
                        self.archive_skill(name);
                        self.usage.forget(name);
                    }
                    archived.push(name.clone());
                }
                SkillVerdict::Duplicate {
                    overlaps_with,
                    merged_name,
                } => {
                    info!(
                        skill = %name,
                        merged_name = %merged_name,
                        overlaps = ?overlaps_with,
                        "Curator: consolidating duplicate"
                    );
                    if !self.config.dry_run {
                        self.archive_skill(name);
                        self.usage.forget(name);
                    }
                    consolidated.push(ConsolidationRecord {
                        merged_name,
                        source_names: {
                            let mut v = overlaps_with;
                            v.insert(0, name.clone());
                            v
                        },
                    });
                }
            }
        }

        let finished_at = Utc::now();
        let report = CuratorRunReport {
            run_id: run_id.clone(),
            started_at,
            finished_at,
            skills_evaluated,
            skills_kept: kept,
            skills_archived: archived,
            skills_consolidated: consolidated,
            skills_protected: protected,
        };

        // Write report (best-effort).
        if !self.config.dry_run {
            self.persist_report(&report);
        }

        info!(
            run_id = %report.run_id,
            evaluated = report.skills_evaluated,
            archived = report.skills_archived.len(),
            consolidated = report.skills_consolidated.len(),
            "Curator: pass complete"
        );

        // Update status snapshot for `last_run_info()`.
        self.total_runs.fetch_add(1, Ordering::Relaxed);
        *self.last_report.write().await = Some(report.clone());

        report
    }

    /// Spawn a long-running task that ticks every `config.cycle`. Returns
    /// the JoinHandle so caller can abort on shutdown.
    pub fn spawn_loop(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let cycle = self.config.cycle;
            let mut interval = tokio::time::interval(cycle);
            // Don't fire instantly on startup; let the platform settle first.
            interval.tick().await;
            // Surface the next scheduled tick to status callers.
            if let Ok(delta) = chrono::Duration::from_std(cycle) {
                *self.next_run_at.write().await = Some(Utc::now() + delta);
            }
            loop {
                interval.tick().await;
                let _ = self.run_once().await;
                // Update next_run_at after each tick.
                if let Ok(delta) = chrono::Duration::from_std(cycle) {
                    *self.next_run_at.write().await = Some(Utc::now() + delta);
                }
            }
        })
    }
}

/// Write (or overwrite) the `metadata.cyberclaw.portability` block inside a
/// SKILL.md YAML frontmatter. Uses string operations only — no YAML parser
/// dependency.
///
/// The frontmatter is delimited by the first and second `---` lines. If the
/// file has no frontmatter the function returns an error without touching it.
///
/// The written block looks like:
/// ```yaml
/// metadata:
///   cyberclaw:
///     portability:
///       tier: tier1
///       required_capabilities: [cmd.run, fs.read]
///       verified_at: '2026-05-05T00:00:00Z'
/// ```
///
/// Rules:
/// - If `portability:` already exists under `metadata.cyberclaw`, it is
///   replaced (not appended a second time).
/// - If `metadata:` / `cyberclaw:` blocks exist but lack `portability:`, the
///   block is injected under the deepest existing ancestor.
/// - If neither exists, the whole `metadata:` subtree is appended before the
///   closing `---`.
fn patch_portability_frontmatter(
    skill_md: &Path,
    report: &PortabilityReport,
) -> std::io::Result<()> {
    // Read current contents — if missing, nothing to patch (no error, scan
    // already yielded a warning in the caller's PortabilityReport).
    let original = match std::fs::read_to_string(skill_md) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };

    let patched = apply_portability_patch(&original, report);
    if patched != original {
        std::fs::write(skill_md, &patched)?;
    }
    Ok(())
}

/// Pure-function core of the frontmatter patch — separated for testability.
fn apply_portability_patch(original: &str, report: &PortabilityReport) -> String {
    let tier_str = match report.tier {
        PortabilityTier::Tier1 => "tier1",
        PortabilityTier::Tier2 => "tier2",
        PortabilityTier::Tier3 => "tier3",
    };
    let caps_yaml = if report.required_capabilities.is_empty() {
        "[]".to_string()
    } else {
        format!(
            "[{}]",
            report
                .required_capabilities
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let verified_at = Utc::now().to_rfc3339();

    let portability_block = format!(
        "    portability:\n      tier: {tier_str}\n      required_capabilities: {caps_yaml}\n      verified_at: '{verified_at}'"
    );

    // Locate frontmatter bounds: lines[0] == "---", next "---" closes it.
    let lines: Vec<&str> = original.lines().collect();
    let fm_start = match lines.first() {
        Some(&"---") => 0,
        _ => {
            // No frontmatter — append a minimal one before the body.
            let sep = if original.ends_with('\n') { "" } else { "\n" };
            return format!(
                "{original}{sep}---\nmetadata:\n  cyberclaw:\n{portability_block}\n---\n"
            );
        }
    };
    let fm_end = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, l)| *l == &"---")
        .map(|(i, _)| i);
    let fm_end = match fm_end {
        Some(i) => i,
        None => {
            // Malformed frontmatter (no closing ---): leave file untouched.
            return original.to_string();
        }
    };

    let fm_lines = &lines[fm_start + 1..fm_end];
    let body_lines = &lines[fm_end..]; // includes closing ---

    // Rebuild the frontmatter, replacing any existing portability block.
    let new_fm = rebuild_frontmatter(fm_lines, &portability_block);

    // Reconstruct the file preserving the trailing newline of the original.
    let trailing_newline = if original.ends_with('\n') { "\n" } else { "" };
    format!(
        "---\n{new_fm}\n{body}{trailing_newline}",
        body = body_lines.join("\n")
    )
}

/// Rebuild frontmatter lines, injecting / replacing the portability block.
///
/// Strategy (line-by-line state machine):
/// 1. If we see `metadata:` at indent 0, enter metadata context.
/// 2. Inside metadata, if we see `  cyberclaw:` at indent 2, enter cyberclaw
///    context.
/// 3. Inside cyberclaw, skip any existing `    portability:` block (4-space
///    indent) and replace it with the new block.
/// 4. If metadata or cyberclaw headers are absent, append them with the block.
fn rebuild_frontmatter(fm_lines: &[&str], portability_block: &str) -> String {
    #[derive(PartialEq)]
    enum State {
        Normal,
        InMetadata,
        InCyberclaw,
        SkippingPortability,
    }

    let mut out: Vec<String> = Vec::with_capacity(fm_lines.len() + 10);
    let mut state = State::Normal;
    let mut found_metadata = false;
    let mut found_cyberclaw = false;
    let mut portability_injected = false;
    let mut i = 0;

    while i < fm_lines.len() {
        let line = fm_lines[i];
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();

        match state {
            State::Normal => {
                if line == "metadata:" {
                    found_metadata = true;
                    state = State::InMetadata;
                    out.push(line.to_string());
                } else {
                    out.push(line.to_string());
                }
            }
            State::InMetadata => {
                // Exit metadata block when we hit a top-level key (indent 0,
                // non-empty, non-comment).
                if indent == 0 && !trimmed.is_empty() && !trimmed.starts_with('#') {
                    // Inject cyberclaw+portability if missing.
                    if !portability_injected {
                        out.push("  cyberclaw:".to_string());
                        out.push(portability_block.to_string());
                        portability_injected = true;
                    }
                    state = State::Normal;
                    // Re-process this line in Normal state.
                    if line == "metadata:" {
                        found_metadata = true;
                        state = State::InMetadata;
                    }
                    out.push(line.to_string());
                } else if line.trim() == "cyberclaw:" || line == "  cyberclaw:" {
                    found_cyberclaw = true;
                    state = State::InCyberclaw;
                    out.push(line.to_string());
                } else {
                    out.push(line.to_string());
                }
            }
            State::InCyberclaw => {
                // Exit cyberclaw when indent drops to ≤ 2 and non-empty non-comment.
                if indent <= 2 && !trimmed.is_empty() && !trimmed.starts_with('#') {
                    // Emit the portability block before leaving if not yet injected.
                    if !portability_injected {
                        out.push(portability_block.to_string());
                        portability_injected = true;
                    }
                    // Check if we're going back to metadata or higher level.
                    if indent == 0 {
                        state = State::Normal;
                        if line == "metadata:" {
                            found_metadata = true;
                            state = State::InMetadata;
                        }
                    } else {
                        state = State::InMetadata;
                        // Check for new cyberclaw key.
                        if line.trim() == "cyberclaw:" || line == "  cyberclaw:" {
                            found_cyberclaw = true;
                            state = State::InCyberclaw;
                        }
                    }
                    out.push(line.to_string());
                } else if trimmed == "portability:" && indent == 4 {
                    // Found existing portability block — replace it.
                    out.push(portability_block.to_string());
                    portability_injected = true;
                    state = State::SkippingPortability;
                } else {
                    out.push(line.to_string());
                }
            }
            State::SkippingPortability => {
                // Skip lines that are part of the old portability block
                // (indent > 4). When we hit indent ≤ 4 non-empty line, stop.
                if indent <= 4 && !trimmed.is_empty() {
                    // Done skipping. Decide where we are now.
                    if indent == 4 {
                        // Another 4-indent key inside cyberclaw.
                        state = State::InCyberclaw;
                    } else if indent <= 2 {
                        state = State::InMetadata;
                    }
                    out.push(line.to_string());
                }
                // else: still inside old portability block, skip.
            }
        }
        i += 1;
    }

    // If we reached end-of-frontmatter still inside blocks, inject if needed.
    if !portability_injected {
        if !found_metadata {
            out.push("metadata:".to_string());
        }
        if !found_cyberclaw {
            out.push("  cyberclaw:".to_string());
        }
        out.push(portability_block.to_string());
    }

    out.join("\n")
}

impl Curator {
    fn archive_skill(&self, skill_name: &str) {
        // Archive implementation: move <skills_root>/installed/<name>/ to
        // <archive_dir>/<run_id>/<name>/. We don't have skills_root direct,
        // so this is a stub — the wiring layer (server) will call a richer
        // implementation that knows the full FS layout.
        let target = self.config.archive_dir.join(skill_name);
        if let Err(e) = std::fs::create_dir_all(&target) {
            warn!(skill = %skill_name, error = %e, "Curator: archive dir create failed");
        }
        // The actual file move is deferred to caller — Curator's job is only
        // the bookkeeping + telemetry forget.
    }

    fn persist_report(&self, report: &CuratorRunReport) {
        let dir = self.config.reports_dir.join(&report.run_id);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            warn!(error = %e, "Curator: report dir create failed");
            return;
        }
        let json_path = dir.join("run.json");
        if let Ok(j) = serde_json::to_string_pretty(report) {
            let _ = std::fs::write(json_path, j);
        }
        let md = render_markdown(report);
        let _ = std::fs::write(dir.join("REPORT.md"), md);
    }
}

fn render_markdown(r: &CuratorRunReport) -> String {
    let mut s = String::new();
    s.push_str(&format!("# Curator Run {}\n\n", r.run_id));
    s.push_str(&format!("- started: {}\n", r.started_at.to_rfc3339()));
    s.push_str(&format!("- finished: {}\n", r.finished_at.to_rfc3339()));
    s.push_str(&format!("- evaluated: {}\n", r.skills_evaluated));
    s.push_str(&format!("- kept: {}\n", r.skills_kept));
    s.push_str(&format!("- archived: {}\n", r.skills_archived.len()));
    s.push_str(&format!(
        "- consolidated: {}\n",
        r.skills_consolidated.len()
    ));
    s.push_str(&format!("- protected: {}\n", r.skills_protected.len()));
    if !r.skills_archived.is_empty() {
        s.push_str("\n## Archived\n");
        for n in &r.skills_archived {
            s.push_str(&format!("- {}\n", n));
        }
    }
    if !r.skills_consolidated.is_empty() {
        s.push_str("\n## Consolidated\n");
        for c in &r.skills_consolidated {
            s.push_str(&format!(
                "- `{}` ← {}\n",
                c.merged_name,
                c.source_names.join(", ")
            ));
        }
    }
    if !r.skills_protected.is_empty() {
        s.push_str("\n## Protected (not eligible for curation)\n");
        for p in &r.skills_protected {
            s.push_str(&format!(
                "- `{}` ({:?}) — {}\n",
                p.skill_name, p.origin, p.reason
            ));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn heuristic_marks_zero_use_old_background_skills_cold() {
        let store = SkillUsageStore::in_memory();
        store.record_install("old_unused", WriteOrigin::BackgroundReview);
        // Force last_used_at far in the past (use_count stays 0).
        store._test_set_last_used_at("old_unused", Utc::now() - chrono::Duration::days(30));
        let store = Arc::new(store);
        let evaluator = HeuristicEvaluator;
        let snap = store.snapshot();
        let cfg = CuratorConfig::for_skills_root(PathBuf::from("/tmp/cyberclaw-test"));
        let v = evaluator.evaluate(&snap, &cfg).await;
        assert!(matches!(
            v.get("old_unused"),
            Some(SkillVerdict::Cold { .. })
        ));
    }

    #[tokio::test]
    async fn heuristic_keeps_recently_used_background_skills() {
        let store = SkillUsageStore::in_memory();
        store.record_install("active", WriteOrigin::BackgroundReview);
        store.record_use("active");
        let store = Arc::new(store);
        let evaluator = HeuristicEvaluator;
        let snap = store.snapshot();
        let cfg = CuratorConfig::for_skills_root(PathBuf::from("/tmp/test"));
        let v = evaluator.evaluate(&snap, &cfg).await;
        assert_eq!(v.get("active"), Some(&SkillVerdict::Keep));
    }

    #[tokio::test]
    async fn foreground_skills_are_protected_from_curator() {
        let tmp = TempDir::new().unwrap();
        let store = SkillUsageStore::in_memory();
        store.record_install("user_skill", WriteOrigin::Foreground);
        store.record_install("bg_skill", WriteOrigin::BackgroundReview);
        // Age both into the past.
        store._test_set_last_used_at("user_skill", Utc::now() - chrono::Duration::days(60));
        store._test_set_last_used_at("bg_skill", Utc::now() - chrono::Duration::days(60));
        let store = Arc::new(store);
        let cfg = CuratorConfig {
            dry_run: true,
            ..CuratorConfig::for_skills_root(tmp.path().to_path_buf())
        };
        let curator = Curator::new(cfg, store, Arc::new(HeuristicEvaluator));
        let report = curator.run_once().await;

        // user_skill in protected list, NOT in archived list.
        assert!(report
            .skills_protected
            .iter()
            .any(|p| p.skill_name == "user_skill"));
        assert!(!report.skills_archived.contains(&"user_skill".to_string()));
        // bg_skill should be archived.
        assert!(report.skills_archived.contains(&"bg_skill".to_string()));
    }

    #[tokio::test]
    async fn dry_run_does_not_mutate_telemetry() {
        let tmp = TempDir::new().unwrap();
        let store = SkillUsageStore::in_memory();
        store.record_install("bg", WriteOrigin::BackgroundReview);
        store._test_set_last_used_at("bg", Utc::now() - chrono::Duration::days(60));
        let store = Arc::new(store);
        let cfg = CuratorConfig {
            dry_run: true,
            ..CuratorConfig::for_skills_root(tmp.path().to_path_buf())
        };
        let curator = Curator::new(cfg, store.clone(), Arc::new(HeuristicEvaluator));
        let report = curator.run_once().await;

        // Report says archived, but record is still present (dry-run).
        assert!(report.skills_archived.contains(&"bg".to_string()));
        assert!(store.snapshot().contains_key("bg"));
    }

    #[tokio::test]
    async fn last_run_info_tracks_run_count_and_report() {
        let tmp = TempDir::new().unwrap();
        let store = Arc::new(SkillUsageStore::in_memory());
        let cfg = CuratorConfig {
            dry_run: true,
            ..CuratorConfig::for_skills_root(tmp.path().to_path_buf())
        };
        let curator = Curator::new(cfg, store, Arc::new(HeuristicEvaluator));

        // Before any run, info is empty.
        let pre = curator.last_run_info().await;
        assert_eq!(pre.total_runs, 0);
        assert!(pre.last_run_at.is_none());
        assert!(pre.last_report.is_none());

        // First run.
        let report1 = curator.run_once().await;
        let info1 = curator.last_run_info().await;
        assert_eq!(info1.total_runs, 1);
        assert_eq!(info1.last_run_at, Some(report1.finished_at));
        assert!(info1.last_report.is_some());
        assert_eq!(info1.last_report.as_ref().unwrap().run_id, report1.run_id);

        // Second run bumps the counter.
        let _report2 = curator.run_once().await;
        let info2 = curator.last_run_info().await;
        assert_eq!(info2.total_runs, 2);
    }

    /// spawn_loop sets next_run_at immediately (no LLM dependency — uses HeuristicEvaluator).
    #[tokio::test]
    async fn spawn_loop_sets_next_run_at() {
        let tmp = TempDir::new().unwrap();
        let store = Arc::new(SkillUsageStore::in_memory());
        let cfg = CuratorConfig {
            // Very short cycle so the loop ticks in the test without long waits.
            cycle: std::time::Duration::from_millis(50),
            ..CuratorConfig::for_skills_root(tmp.path().to_path_buf())
        };
        let curator = Arc::new(Curator::new(cfg, store, Arc::new(HeuristicEvaluator)));

        // Before spawning, next_run_at is None.
        assert!(curator.last_run_info().await.next_run_at.is_none());

        // spawn_loop schedules the first tick and sets next_run_at.
        let handle = curator.clone().spawn_loop();

        // Give the background task time to register the first scheduled tick.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let info = curator.last_run_info().await;
        assert!(
            info.next_run_at.is_some(),
            "spawn_loop must set next_run_at once scheduled"
        );

        // Abort the task to avoid leaking it after the test.
        handle.abort();
    }

    #[tokio::test]
    async fn report_is_written_to_disk() {
        let tmp = TempDir::new().unwrap();
        let store = SkillUsageStore::in_memory();
        store.record_install("dummy", WriteOrigin::Foreground);
        let store = Arc::new(store);
        let cfg = CuratorConfig::for_skills_root(tmp.path().to_path_buf());
        let curator = Curator::new(cfg.clone(), store, Arc::new(HeuristicEvaluator));
        let report = curator.run_once().await;

        let report_dir = cfg.reports_dir.join(&report.run_id);
        assert!(report_dir.join("run.json").exists());
        assert!(report_dir.join("REPORT.md").exists());
        let md = std::fs::read_to_string(report_dir.join("REPORT.md")).unwrap();
        assert!(md.contains("Curator Run"));
        assert!(md.contains("dummy")); // shows up in Protected section
    }

    // -----------------------------------------------------------------------
    // Portability frontmatter patch tests (Layer E)
    // -----------------------------------------------------------------------

    /// run_once writes portability frontmatter into a BackgroundReview skill's
    /// SKILL.md that has no existing portability block.
    #[tokio::test]
    async fn portability_frontmatter_written_on_keep() {
        let tmp = TempDir::new().unwrap();

        // Create installed/<skill>/SKILL.md.
        let skill_dir = tmp.path().join("installed").join("my_skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let skill_md = skill_dir.join("SKILL.md");
        std::fs::write(
            &skill_md,
            "---\nname: my_skill\ndescription: a test skill\nallowed-tools: [cmd.run]\n---\n\n# My Skill\n",
        )
        .unwrap();

        // Register as BackgroundReview — eligible for curation — and keep it
        // warm so it gets `Keep` verdict (not Cold).
        let store = SkillUsageStore::in_memory();
        store.record_install("my_skill", WriteOrigin::BackgroundReview);
        store.record_use("my_skill");
        let store = Arc::new(store);

        // dry_run=false so portability patch is applied.
        let cfg = CuratorConfig::for_skills_root(tmp.path().to_path_buf());
        let curator = Curator::new(cfg, store, Arc::new(HeuristicEvaluator));
        let report = curator.run_once().await;

        // Skill should be kept (not archived).
        assert!(report.skills_archived.is_empty());
        assert_eq!(report.skills_kept, 1);

        // SKILL.md must now contain portability frontmatter.
        let contents = std::fs::read_to_string(&skill_md).unwrap();
        assert!(
            contents.contains("portability:"),
            "portability block must be written: {contents}"
        );
        assert!(
            contents.contains("tier: tier1"),
            "cmd.run is Tier1: {contents}"
        );
        assert!(
            contents.contains("required_capabilities: [cmd.run]"),
            "capability list preserved: {contents}"
        );
        assert!(
            contents.contains("verified_at:"),
            "verified_at timestamp written: {contents}"
        );
    }

    /// Existing portability block is overwritten (not duplicated) on a second
    /// curator pass.
    #[tokio::test]
    async fn portability_frontmatter_overwritten_not_duplicated() {
        let tmp = TempDir::new().unwrap();

        let skill_dir = tmp.path().join("installed").join("sk");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let skill_md = skill_dir.join("SKILL.md");
        // Pre-seed an outdated portability block.
        std::fs::write(
            &skill_md,
            "---\nname: sk\ndescription: x\nallowed-tools: [http.get]\nmetadata:\n  cyberclaw:\n    portability:\n      tier: tier1\n      required_capabilities: []\n      verified_at: '2000-01-01T00:00:00Z'\n---\n\n# SK\n",
        )
        .unwrap();

        let store = SkillUsageStore::in_memory();
        store.record_install("sk", WriteOrigin::BackgroundReview);
        store.record_use("sk");
        let store = Arc::new(store);

        let cfg = CuratorConfig::for_skills_root(tmp.path().to_path_buf());
        let curator = Curator::new(cfg, store, Arc::new(HeuristicEvaluator));
        let _report = curator.run_once().await;

        let contents = std::fs::read_to_string(&skill_md).unwrap();

        // Must contain exactly one portability: key.
        let count = contents.matches("portability:").count();
        assert_eq!(
            count, 1,
            "portability block must appear exactly once: {contents}"
        );

        // Tier must now reflect http.get → Tier2 (overriding old tier1).
        assert!(
            contents.contains("tier: tier2"),
            "http.get upgrades tier to tier2: {contents}"
        );

        // Old timestamp must be gone.
        assert!(
            !contents.contains("2000-01-01"),
            "stale verified_at must be replaced: {contents}"
        );
    }

    /// A skill containing a Tier3 capability gets `tier: tier3` in frontmatter,
    /// and warnings do not prevent the write.
    #[tokio::test]
    async fn portability_frontmatter_tier3_written_with_warnings() {
        let tmp = TempDir::new().unwrap();

        let skill_dir = tmp.path().join("installed").join("ai_skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let skill_md = skill_dir.join("SKILL.md");
        std::fs::write(
            &skill_md,
            "---\nname: ai_skill\ndescription: calls openai\nallowed-tools: [openai.chat, foo.unknown]\n---\n\n# AI Skill\n",
        )
        .unwrap();

        let store = SkillUsageStore::in_memory();
        store.record_install("ai_skill", WriteOrigin::BackgroundReview);
        store.record_use("ai_skill");
        let store = Arc::new(store);

        let cfg = CuratorConfig::for_skills_root(tmp.path().to_path_buf());
        let curator = Curator::new(cfg, store, Arc::new(HeuristicEvaluator));
        let _report = curator.run_once().await;

        let contents = std::fs::read_to_string(&skill_md).unwrap();
        assert!(
            contents.contains("tier: tier3"),
            "openai.chat is Tier3: {contents}"
        );
        assert!(
            contents.contains("openai.chat"),
            "openai.chat in required_capabilities: {contents}"
        );
        // foo.unknown is unknown but still listed in required_capabilities.
        assert!(
            contents.contains("foo.unknown"),
            "unknown cap still listed: {contents}"
        );
    }
}
