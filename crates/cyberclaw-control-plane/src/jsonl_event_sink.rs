//! Append-only JSONL persistence for [`EvolutionCycleSummary`].
//!
//! Mirrors Evolver's `events.jsonl` managed by `assetStore.js:191-194`
//! (`appendEventJsonl`). Each cycle appends one line; the file is the
//! audit substrate that [`crate::history_analyzer`] reads back to close
//! the feedback loop.
//!
//! # Why JSONL
//!
//! - Append-only → crash recovery is trivial (a partial tail line is
//!   skipped by `load_all`).
//! - Streamable → `HistoryAnalyzer` only needs the tail, no whole-file
//!   parse required.
//! - Toolable → any line-based tool (`tail`, `jq`, `grep`) inspects the
//!   trail.
//!
//! P1 will migrate to `cyberclaw-observability`'s structured event store
//! (see architect review §6). Until then, JSONL is the stepping stone.

use std::fs::OpenOptions;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::cycle_summary::EvolutionCycleSummary;

/// Thread-safe append-only JSONL sink.
pub struct JsonlCycleSink {
    path: PathBuf,
    inner: Mutex<BufWriter<std::fs::File>>,
}

impl JsonlCycleSink {
    /// Open (or create) the given JSONL file in append mode. The parent
    /// directory must already exist — intentional, to surface path typos
    /// early instead of silently materializing stray directories.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            inner: Mutex::new(BufWriter::new(file)),
        })
    }

    /// Append one summary and flush to disk. Returns once the underlying
    /// `Write` is flushed (process-durable; not fsync-durable — acceptable
    /// for audit telemetry, not for transactional state).
    pub fn record(&self, summary: &EvolutionCycleSummary) -> io::Result<()> {
        let mut line = serde_json::to_vec(summary)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        line.push(b'\n');
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.write_all(&line)?;
        guard.flush()?;
        Ok(())
    }

    /// Absolute path to the JSONL file. Handy for diagnostics + read-back.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read every summary back from the file (oldest first). Lines that
    /// fail to parse are silently skipped — this is the "partial tail
    /// after crash" recovery rule from Evolver's reader semantics.
    ///
    /// Missing file → returns an empty `Vec`, not an error.
    pub fn load_all(path: impl AsRef<Path>) -> io::Result<Vec<EvolutionCycleSummary>> {
        let content = match std::fs::read_to_string(path.as_ref()) {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(summary) = serde_json::from_str::<EvolutionCycleSummary>(trimmed) {
                out.push(summary);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cycle_summary::{
        BlastRadius, CycleIntent, CycleOutcome, CycleStatus, EvolutionCycleSummary,
    };
    use chrono::Utc;
    use tempfile::NamedTempFile;

    fn sample(id: &str) -> EvolutionCycleSummary {
        EvolutionCycleSummary {
            id: id.to_string(),
            timestamp: Utc::now(),
            intent: CycleIntent::Repair,
            signals: vec!["error".to_string(), "recurring_errsig:divzero".to_string()],
            outcome: CycleOutcome {
                status: CycleStatus::Success,
                score: Some(0.83),
                error: None,
            },
            blast_radius: BlastRadius {
                files: 2,
                lines: 41,
            },
            variant_id: Some("v_abc123".to_string()),
            genes_used: vec!["gene_gep_repair_from_errors".to_string()],
            meta: serde_json::Map::new(),
        }
    }

    #[test]
    fn write_then_load_roundtrip() {
        let tmp = NamedTempFile::new().unwrap();
        let sink = JsonlCycleSink::open(tmp.path()).unwrap();
        for i in 0..3 {
            sink.record(&sample(&format!("cycle_{i}"))).unwrap();
        }

        let loaded = JsonlCycleSink::load_all(tmp.path()).unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].id, "cycle_0");
        assert_eq!(loaded[2].id, "cycle_2");
        assert_eq!(loaded[1].signals.len(), 2);
    }

    #[test]
    fn load_missing_file_is_empty() {
        let path = std::env::temp_dir().join(format!(
            "cyberclaw_jsonl_not_exist_{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let loaded = JsonlCycleSink::load_all(&path).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn load_skips_corrupt_tail_line() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        {
            let sink = JsonlCycleSink::open(&path).unwrap();
            sink.record(&sample("cycle_0")).unwrap();
        }
        // Simulate crash: partial JSON tail line.
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{{not_valid_json").unwrap();
        drop(f);

        let loaded = JsonlCycleSink::load_all(&path).unwrap();
        assert_eq!(loaded.len(), 1, "corrupt tail must be skipped");
    }

    #[test]
    fn concurrent_records_serialize_correctly() {
        use std::sync::Arc;
        use std::thread;

        let tmp = NamedTempFile::new().unwrap();
        let sink = Arc::new(JsonlCycleSink::open(tmp.path()).unwrap());
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let s = sink.clone();
                thread::spawn(move || {
                    s.record(&sample(&format!("t_{i}"))).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let loaded = JsonlCycleSink::load_all(tmp.path()).unwrap();
        assert_eq!(loaded.len(), 8, "all concurrent writes must persist");
    }
}
