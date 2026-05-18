//! Evolution daemon — the I-Daemon invariant.
//!
//! Runs an autonomous loop that:
//!
//! 1. Loads past cycle summaries from the JSONL audit log.
//! 2. Computes [`HistoryAnalysis`] (suppression, stagnation, failure streaks).
//! 3. Extracts signals via [`signal_extractor`].
//! 4. Routes signals to a [`EvolutionGene`] via [`signal_router`].
//! 5. Records the decision back to the JSONL log.
//! 6. Sleeps with adaptive backoff and respects a kill switch.
//!
//! # Scope (P0 Step 1)
//!
//! The daemon is intentionally **dry-run only**: it records the decision
//! but does NOT invoke the real [`crate::evolution_orchestrator::EvolutionOrchestrator`].
//! Wiring the orchestrator is P1 work so it can be audited separately.
//! Every recorded summary has `outcome.status == Skipped` and
//! `meta.dry_run == true`, making dry-run cycles trivially filterable.
//!
//! Governance invariant: the daemon itself performs zero Connector /
//! Capability calls. It only reads/writes the audit log. When real
//! mutation is added in P1 it will go through `EvolutionDispatcher`,
//! which already enforces the Connector → Capability chain.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::time::Instant;
use uuid::Uuid;

use crate::cycle_summary::{
    BlastRadius, CycleIntent, CycleOutcome, CycleStatus, EvolutionCycleSummary,
};
use crate::evolution_gene::EvolutionGene;
use crate::history_analyzer::{analyze_recent, DEFAULT_HISTORY_WINDOW};
use crate::jsonl_event_sink::JsonlCycleSink;
use crate::signal_extractor::{extract_signals, ExtractorConfig, ExtractorInput};
use crate::signal_router::{route, RouterConfig, RoutingDecision};

/// Why the daemon stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonStopReason {
    /// Kill switch was set externally.
    KillSwitch,
    /// Cycle budget exhausted — caller should respawn if desired (mirrors
    /// Evolver `index.js:315-335` suicide-and-restart pattern).
    CycleBudget,
}

/// Daemon configuration — one-shot, owned by the daemon task.
pub struct DaemonConfig {
    /// Where to write/read cycle summaries.
    pub jsonl_path: PathBuf,
    /// Genes available for selection.
    pub genes: Vec<EvolutionGene>,
    /// Minimum sleep between cycles.
    pub min_sleep: Duration,
    /// Cap on sleep after exponential backoff.
    pub max_sleep: Duration,
    /// Initial sleep — usually the same as `min_sleep`.
    pub initial_sleep: Duration,
    /// Cycles below this wall-time count as "too fast" and trigger backoff
    /// (Evolver `index.js:271-275` uses 500ms).
    pub fast_cycle_threshold: Duration,
    /// Process-level cycle cap for memory-hygiene restarts.
    pub max_cycles_per_process: u32,
    /// External kill signal; set to `true` to request graceful stop.
    pub kill_switch: Arc<AtomicBool>,
}

impl DaemonConfig {
    /// Sensible defaults — 2-second min, 5-minute max, 1000 cycles budget.
    pub fn new_with_defaults(jsonl_path: PathBuf, genes: Vec<EvolutionGene>) -> Self {
        Self {
            jsonl_path,
            genes,
            min_sleep: Duration::from_secs(2),
            max_sleep: Duration::from_secs(300),
            initial_sleep: Duration::from_secs(2),
            fast_cycle_threshold: Duration::from_millis(500),
            max_cycles_per_process: 1000,
            kill_switch: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Per-cycle summary for the daemon's own loop controller.
#[derive(Debug, Clone)]
pub struct CycleOutcomeInternal {
    pub failed: bool,
    #[allow(dead_code)]
    pub decision: RoutingDecision,
}

/// The running daemon. Call [`EvolutionDaemon::run`] to block until
/// stopped.
pub struct EvolutionDaemon {
    config: DaemonConfig,
    sink: JsonlCycleSink,
    extractor_config: ExtractorConfig,
    router_config: RouterConfig,
    current_sleep: Duration,
    cycle_count: u32,
}

impl EvolutionDaemon {
    /// Construct a daemon; opens the JSONL sink eagerly so bad paths fail
    /// fast rather than at first write.
    pub fn new(config: DaemonConfig) -> std::io::Result<Self> {
        let sink = JsonlCycleSink::open(&config.jsonl_path)?;
        let current_sleep = config.initial_sleep;
        Ok(Self {
            config,
            sink,
            extractor_config: ExtractorConfig::default(),
            router_config: RouterConfig::default(),
            current_sleep,
            cycle_count: 0,
        })
    }

    /// Inject a custom [`ExtractorConfig`] — mostly for tests.
    pub fn with_extractor_config(mut self, cfg: ExtractorConfig) -> Self {
        self.extractor_config = cfg;
        self
    }

    /// Inject a custom [`RouterConfig`] — mostly for tests.
    pub fn with_router_config(mut self, cfg: RouterConfig) -> Self {
        self.router_config = cfg;
        self
    }

    /// Read-only access to the current sleep — useful for observability.
    pub fn current_sleep(&self) -> Duration {
        self.current_sleep
    }

    /// How many cycles have been recorded by this instance.
    pub fn cycle_count(&self) -> u32 {
        self.cycle_count
    }

    /// Run until kill switch OR cycle budget exhausted. Returns the reason.
    pub async fn run(&mut self) -> anyhow::Result<DaemonStopReason> {
        loop {
            if self.config.kill_switch.load(Ordering::Relaxed) {
                return Ok(DaemonStopReason::KillSwitch);
            }
            if self.cycle_count >= self.config.max_cycles_per_process {
                return Ok(DaemonStopReason::CycleBudget);
            }

            let cycle_started = Instant::now();
            let outcome = self.run_one_cycle().await?;
            let dt = cycle_started.elapsed();

            self.apply_adaptive_sleep(&outcome, dt);
            self.cycle_count += 1;

            if interruptible_sleep(&self.config.kill_switch, self.current_sleep).await {
                return Ok(DaemonStopReason::KillSwitch);
            }
        }
    }

    /// Execute one cycle: analyze → extract → route → record (dry-run).
    /// Public so callers can drive the loop manually in tests or harnesses.
    pub async fn run_one_cycle(&self) -> anyhow::Result<CycleOutcomeInternal> {
        let summaries = JsonlCycleSink::load_all(&self.config.jsonl_path).unwrap_or_default();
        let history = analyze_recent(&summaries, DEFAULT_HISTORY_WINDOW);

        let signals = extract_signals(
            ExtractorInput {
                history: &history,
                execution_log: None,
                user_input: None,
            },
            &self.extractor_config,
        );

        let decision = route(&signals, &self.config.genes, &history, &self.router_config);

        let mut meta = serde_json::Map::new();
        meta.insert("dry_run".into(), serde_json::Value::Bool(true));
        meta.insert("confidence".into(), serde_json::json!(decision.confidence));
        meta.insert(
            "consecutive_empty_cycles".into(),
            serde_json::json!(history.consecutive_empty_cycles),
        );
        meta.insert(
            "consecutive_failure_count".into(),
            serde_json::json!(history.consecutive_failure_count),
        );

        let summary = EvolutionCycleSummary {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            intent: decision.category.unwrap_or(CycleIntent::Optimize),
            signals,
            outcome: CycleOutcome {
                status: CycleStatus::Skipped,
                score: None,
                error: Some("daemon dry-run: orchestrator wiring pending P1".into()),
            },
            blast_radius: BlastRadius::default(),
            variant_id: None,
            genes_used: decision.selected_gene.clone().into_iter().collect(),
            meta,
        };

        self.sink.record(&summary)?;

        Ok(CycleOutcomeInternal {
            failed: false,
            decision,
        })
    }

    fn apply_adaptive_sleep(&mut self, outcome: &CycleOutcomeInternal, dt: Duration) {
        let too_fast = dt < self.config.fast_cycle_threshold;
        if outcome.failed || too_fast {
            let doubled = self.current_sleep.saturating_mul(2);
            self.current_sleep = doubled.clamp(self.config.min_sleep, self.config.max_sleep);
        } else {
            self.current_sleep = self.config.min_sleep;
        }
    }
}

/// Sleep up to `total`, but wake early if `kill_switch` goes high. Returns
/// `true` if woken by the kill switch.
async fn interruptible_sleep(kill_switch: &Arc<AtomicBool>, total: Duration) -> bool {
    let chunk = Duration::from_millis(250);
    let start = Instant::now();
    loop {
        if kill_switch.load(Ordering::Relaxed) {
            return true;
        }
        let elapsed = start.elapsed();
        if elapsed >= total {
            return false;
        }
        let remaining = total - elapsed;
        let wait = if remaining < chunk { remaining } else { chunk };
        tokio::time::sleep(wait).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution_gene::default_genes;
    use tempfile::NamedTempFile;

    fn make_config(kill: Arc<AtomicBool>, path: PathBuf) -> DaemonConfig {
        DaemonConfig {
            jsonl_path: path,
            genes: default_genes(),
            min_sleep: Duration::from_millis(10),
            max_sleep: Duration::from_millis(200),
            initial_sleep: Duration::from_millis(10),
            fast_cycle_threshold: Duration::from_millis(5),
            max_cycles_per_process: 5,
            kill_switch: kill,
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn budget_stop_records_skipped_dry_run_summaries() {
        let tmp = NamedTempFile::new().unwrap();
        let kill = Arc::new(AtomicBool::new(false));
        let mut daemon =
            EvolutionDaemon::new(make_config(kill.clone(), tmp.path().to_path_buf())).unwrap();

        let reason = daemon.run().await.unwrap();
        assert_eq!(reason, DaemonStopReason::CycleBudget);
        assert_eq!(daemon.cycle_count(), 5);

        let summaries = JsonlCycleSink::load_all(tmp.path()).unwrap();
        assert_eq!(summaries.len(), 5);
        for s in &summaries {
            assert_eq!(s.outcome.status, CycleStatus::Skipped);
            assert_eq!(s.meta.get("dry_run").and_then(|v| v.as_bool()), Some(true));
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn kill_switch_stops_promptly() {
        let tmp = NamedTempFile::new().unwrap();
        let kill = Arc::new(AtomicBool::new(false));
        let mut cfg = make_config(kill.clone(), tmp.path().to_path_buf());
        cfg.min_sleep = Duration::from_millis(200);
        cfg.max_cycles_per_process = 10_000;

        let mut daemon = EvolutionDaemon::new(cfg).unwrap();
        let kill_to_flip = kill.clone();
        let handle = tokio::spawn(async move { daemon.run().await });

        tokio::time::sleep(Duration::from_millis(50)).await;
        kill_to_flip.store(true, Ordering::Relaxed);

        let reason = handle.await.unwrap().unwrap();
        assert_eq!(reason, DaemonStopReason::KillSwitch);
    }

    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn cycle_writes_meta_confidence() {
        let tmp = NamedTempFile::new().unwrap();
        let kill = Arc::new(AtomicBool::new(false));
        let mut cfg = make_config(kill.clone(), tmp.path().to_path_buf());
        cfg.max_cycles_per_process = 1;
        let mut daemon = EvolutionDaemon::new(cfg).unwrap();

        let _ = daemon.run().await.unwrap();
        let summaries = JsonlCycleSink::load_all(tmp.path()).unwrap();
        assert_eq!(summaries.len(), 1);
        assert!(summaries[0].meta.contains_key("confidence"));
        assert!(summaries[0].meta.contains_key("consecutive_empty_cycles"));
    }

    #[tokio::test(flavor = "current_thread", start_paused = false)]
    async fn adaptive_sleep_backs_off_on_fast_cycle() {
        let tmp = NamedTempFile::new().unwrap();
        let kill = Arc::new(AtomicBool::new(false));
        let cfg = DaemonConfig {
            jsonl_path: tmp.path().to_path_buf(),
            genes: default_genes(),
            // Start at 10ms so doubling is observable.
            min_sleep: Duration::from_millis(10),
            max_sleep: Duration::from_millis(1000),
            initial_sleep: Duration::from_millis(10),
            // Force every cycle to count as "fast".
            fast_cycle_threshold: Duration::from_secs(10),
            max_cycles_per_process: 3,
            kill_switch: kill.clone(),
        };
        let mut daemon = EvolutionDaemon::new(cfg).unwrap();
        let _ = daemon.run().await.unwrap();

        // After 3 fast cycles: 10 → 20 → 40 → 80.
        assert!(
            daemon.current_sleep() >= Duration::from_millis(20),
            "expected backoff to have grown, got {:?}",
            daemon.current_sleep()
        );
    }
}
