//! Evolution Gene — named behavioral template (the I-Genome invariant).
//!
//! A [`EvolutionGene`] binds a set of signal patterns to a mutation strategy
//! plus safety constraints. The [`crate::signal_router`] scores incoming
//! signals against each gene's `signals_match` field and returns the best
//! match to drive the next evolution cycle.
//!
//! Shape mirrors Evolver's gene schema at
//! `/tmp/claw-research/evolver/assets/gep/genes.json:1-109`.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::cycle_summary::CycleIntent;

/// Gene-level constraints — upper bounds on blast radius.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneConstraints {
    /// Hard cap on the number of files a single cycle may touch.
    /// Evolver default: 20 (see `genes.json:27`).
    #[serde(default = "default_max_files")]
    pub max_files: u32,
    /// Paths the gene must never touch. Checked before any edit is applied.
    #[serde(default = "default_forbidden_paths")]
    pub forbidden_paths: Vec<String>,
}

impl Default for GeneConstraints {
    fn default() -> Self {
        Self {
            max_files: default_max_files(),
            forbidden_paths: default_forbidden_paths(),
        }
    }
}

fn default_max_files() -> u32 {
    20
}

fn default_forbidden_paths() -> Vec<String> {
    vec![".git".into(), "node_modules".into(), "target".into()]
}

/// A reusable evolution template. Stored in `genes.json` and referenced by
/// ID in [`crate::cycle_summary::EvolutionCycleSummary::genes_used`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvolutionGene {
    /// Stable identifier (e.g. `gene_cyberclaw_repair_from_errors`).
    pub id: String,
    /// Which evolution category this gene implements.
    pub category: CycleIntent,
    /// Signal patterns this gene responds to. A signal matches if it
    /// equals the pattern OR starts with `{pattern}:` (prefix match).
    #[serde(default)]
    pub signals_match: Vec<String>,
    /// Human-readable preconditions (informational; not runtime-checked).
    #[serde(default)]
    pub preconditions: Vec<String>,
    /// Ordered strategy steps. Used as a prompt template in the future
    /// `PromptAssembler`; inert until that lands.
    #[serde(default)]
    pub strategy: Vec<String>,
    /// Safety envelope.
    #[serde(default)]
    pub constraints: GeneConstraints,
    /// Validation capability references. Will be evaluated through
    /// Connector → Capability (see CLAUDE.md §3). Raw shell commands are
    /// explicitly **not** supported — this is why the field is opaque strings.
    #[serde(default)]
    pub validation: Vec<String>,
}

/// Envelope file format (matches Evolver's `genes.json:1-4`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneFile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub genes: Vec<EvolutionGene>,
}

fn default_version() -> u32 {
    2
}

/// Load genes from a JSON file. Missing file returns an empty list — the
/// daemon can still run on [`default_genes`] alone.
pub fn load_genes(path: impl AsRef<Path>) -> io::Result<Vec<EvolutionGene>> {
    let content = match std::fs::read_to_string(path.as_ref()) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let file: GeneFile = serde_json::from_str(&content)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(file.genes)
}

/// Built-in default genes — a CyberClaw adaptation of Evolver's three
/// canonical genes at `genes.json:4-108`. Enough to bootstrap the daemon
/// before any on-disk gene library exists.
pub fn default_genes() -> Vec<EvolutionGene> {
    vec![
        EvolutionGene {
            id: "gene_cyberclaw_repair_from_errors".into(),
            category: CycleIntent::Repair,
            signals_match: vec![
                "error".into(),
                "exception".into(),
                "failed".into(),
                "unstable".into(),
                "errsig".into(),
                "consecutive_failures".into(),
            ],
            preconditions: vec!["signals contain error-related indicators".into()],
            strategy: vec![
                "Extract structured signals from execution history".into(),
                "Estimate blast radius before editing".into(),
                "Apply smallest reversible patch".into(),
                "Validate via declared validation capabilities".into(),
                "Solidify: write EvolutionCycleSummary".into(),
            ],
            constraints: GeneConstraints::default(),
            validation: Vec::new(),
        },
        EvolutionGene {
            id: "gene_cyberclaw_optimize_prompt_and_assets".into(),
            category: CycleIntent::Optimize,
            signals_match: vec![
                "protocol".into(),
                "prompt".into(),
                "audit".into(),
                "reusable".into(),
                "user_improvement_suggestion".into(),
            ],
            preconditions: vec!["need stricter, auditable evolution outputs".into()],
            strategy: vec![
                "Prefer reusing existing Gene / past Summary over creating new".into(),
                "Refactor prompt assembly to embed recent history".into(),
                "Reduce noise and ambiguity".into(),
                "Solidify: record cycle summary".into(),
            ],
            constraints: GeneConstraints::default(),
            validation: Vec::new(),
        },
        EvolutionGene {
            id: "gene_cyberclaw_innovate_from_opportunity".into(),
            category: CycleIntent::Innovate,
            signals_match: vec![
                "user_feature_request".into(),
                "perf_bottleneck".into(),
                "capability_gap".into(),
                "stable_success_plateau".into(),
                "force_innovation_after_repair_loop".into(),
                "evolution_stagnation_detected".into(),
            ],
            preconditions: vec![
                "at least one opportunity signal is present".into(),
                "no active repair loop".into(),
            ],
            strategy: vec![
                "Identify the specific user need or system gap".into(),
                "Search existing Genes for partial matches".into(),
                "Design a minimal testable implementation plan".into(),
                "Solidify: record cycle summary with intent=innovate".into(),
            ],
            constraints: GeneConstraints {
                max_files: 25,
                ..GeneConstraints::default()
            },
            validation: Vec::new(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn default_genes_cover_all_three_intents() {
        let g = default_genes();
        assert_eq!(g.len(), 3);
        assert!(g.iter().any(|x| matches!(x.category, CycleIntent::Repair)));
        assert!(g
            .iter()
            .any(|x| matches!(x.category, CycleIntent::Optimize)));
        assert!(g
            .iter()
            .any(|x| matches!(x.category, CycleIntent::Innovate)));
    }

    #[test]
    fn default_constraints_forbid_git_and_target() {
        let c = GeneConstraints::default();
        assert!(c.forbidden_paths.iter().any(|p| p == ".git"));
        assert!(c.forbidden_paths.iter().any(|p| p == "target"));
    }

    #[test]
    fn load_genes_roundtrip() {
        let tmp = NamedTempFile::new().unwrap();
        let original = default_genes();
        let envelope = GeneFile {
            version: 2,
            genes: original.clone(),
        };
        std::fs::write(tmp.path(), serde_json::to_string(&envelope).unwrap()).unwrap();

        let loaded = load_genes(tmp.path()).unwrap();
        assert_eq!(loaded, original);
    }

    #[test]
    fn load_genes_missing_file_is_empty() {
        let path = std::env::temp_dir().join(format!(
            "cyberclaw_genes_missing_{}.json",
            uuid::Uuid::new_v4()
        ));
        assert!(load_genes(&path).unwrap().is_empty());
    }

    #[test]
    fn load_genes_rejects_malformed() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "{ not valid json").unwrap();
        assert!(load_genes(tmp.path()).is_err());
    }

    #[test]
    fn gene_serializes_with_snake_case_category() {
        let g = &default_genes()[0];
        let s = serde_json::to_string(g).unwrap();
        assert!(s.contains("\"category\":\"repair\""));
    }
}
