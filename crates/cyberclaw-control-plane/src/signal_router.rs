//! Signal router — the I-Selection invariant.
//!
//! Scores each [`EvolutionGene`] against the incoming signal set and picks
//! the best match. Returns not just the winner but also the runners-up and
//! a "drift intensity" score that quantifies how close the race was (useful
//! for exploration vs exploitation tuning later).
//!
//! Approximate of Evolver's `selector.js` scoring (selector.test.js:60-239),
//! but without the personality / capsule layers — those are P2+ material.

use std::collections::HashSet;

use crate::cycle_summary::CycleIntent;
use crate::evolution_gene::EvolutionGene;
use crate::history_analyzer::HistoryAnalysis;

/// The router's verdict for one cycle.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RoutingDecision {
    /// Chosen gene ID. `None` when no gene scored above zero.
    pub selected_gene: Option<String>,
    /// Chosen gene's intent category. `None` mirrors [`selected_gene`].
    pub category: Option<CycleIntent>,
    /// Raw match count of the winner.
    pub score: u32,
    /// How close the second-best was, in `[0.0, 1.0]`. Higher = wider
    /// margin / more confident pick; lower = genes tied and exploration is
    /// advised. `1.0` on a unique match, `0.0` on a perfect tie.
    pub confidence: f32,
    /// Top-K runners up (id, score).
    pub alternatives: Vec<(String, u32)>,
}

/// Router configuration — currently only `alt_limit` is tunable.
pub struct RouterConfig {
    /// How many alternatives to return alongside the winner. Cap for UI /
    /// debugging; does not affect selection.
    pub alt_limit: usize,
    /// Multiplicative penalty applied to a gene's score for each of its
    /// `signals_match` entries that is present in
    /// [`HistoryAnalysis::suppressed_signals`]. `0.0` = fully suppressed,
    /// `1.0` = no effect. Default `0.5`.
    pub suppression_penalty: f32,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            alt_limit: 3,
            suppression_penalty: 0.5,
        }
    }
}

/// Score and pick one gene. `history` can be
/// [`HistoryAnalysis::default`] when no history is available — suppression
/// then has no effect.
pub fn route(
    signals: &[String],
    genes: &[EvolutionGene],
    history: &HistoryAnalysis,
    config: &RouterConfig,
) -> RoutingDecision {
    if signals.is_empty() || genes.is_empty() {
        return RoutingDecision::default();
    }

    let signal_set: HashSet<&str> = signals.iter().map(String::as_str).collect();

    // Score each gene: +1 per signals_match entry that matches any incoming
    // signal (exact or `{pattern}:` prefix). Then apply suppression penalty.
    let mut scored: Vec<(usize, f32, u32)> = genes
        .iter()
        .enumerate()
        .map(|(idx, g)| {
            let mut hits: u32 = 0;
            let mut suppressed_hits: u32 = 0;
            for pat in &g.signals_match {
                if signal_set.iter().any(|s| signal_matches(s, pat)) {
                    hits += 1;
                    if history.suppressed_signals.contains(pat) {
                        suppressed_hits += 1;
                    }
                }
            }
            let adj = hits as f32 - suppressed_hits as f32 * (1.0 - config.suppression_penalty);
            (idx, adj, hits)
        })
        .filter(|(_, adj, _)| *adj > 0.0)
        .collect();

    // Sort by adjusted score DESC, then by gene id ASC for stable tiebreak.
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| genes[a.0].id.cmp(&genes[b.0].id))
    });

    let Some(&(best_idx, _best_adj, best_hits)) = scored.first() else {
        return RoutingDecision::default();
    };

    let best = &genes[best_idx];
    let confidence = if scored.len() >= 2 {
        let second_adj = scored[1].1;
        let best_adj = scored[0].1;
        if best_adj > 0.0 {
            ((best_adj - second_adj) / best_adj).clamp(0.0, 1.0)
        } else {
            0.0
        }
    } else {
        1.0
    };

    let alternatives: Vec<(String, u32)> = scored
        .iter()
        .skip(1)
        .take(config.alt_limit)
        .map(|&(idx, _adj, h)| (genes[idx].id.clone(), h))
        .collect();

    RoutingDecision {
        selected_gene: Some(best.id.clone()),
        category: Some(best.category),
        score: best_hits,
        confidence,
        alternatives,
    }
}

/// A signal matches a `signals_match` pattern iff it equals the pattern
/// exactly OR begins with `{pattern}:` (detail suffix).
fn signal_matches(signal: &str, pattern: &str) -> bool {
    if signal == pattern {
        return true;
    }
    if let Some(rest) = signal.strip_prefix(pattern) {
        return rest.starts_with(':');
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution_gene::default_genes;

    #[test]
    fn empty_signals_returns_no_selection() {
        let d = route(
            &[],
            &default_genes(),
            &HistoryAnalysis::default(),
            &RouterConfig::default(),
        );
        assert!(d.selected_gene.is_none());
        assert!(d.category.is_none());
    }

    #[test]
    fn empty_genes_returns_no_selection() {
        let d = route(
            &["error".into()],
            &[],
            &HistoryAnalysis::default(),
            &RouterConfig::default(),
        );
        assert!(d.selected_gene.is_none());
    }

    #[test]
    fn error_signal_picks_repair_gene() {
        let d = route(
            &["error".into(), "failed".into()],
            &default_genes(),
            &HistoryAnalysis::default(),
            &RouterConfig::default(),
        );
        assert_eq!(d.category, Some(CycleIntent::Repair));
        assert!(d.selected_gene.as_deref().unwrap().contains("repair"));
    }

    #[test]
    fn feature_request_picks_innovate_gene() {
        let d = route(
            &["user_feature_request".into()],
            &default_genes(),
            &HistoryAnalysis::default(),
            &RouterConfig::default(),
        );
        assert_eq!(d.category, Some(CycleIntent::Innovate));
    }

    #[test]
    fn prefix_match_works() {
        // "errsig:divzero" should match a pattern of "errsig".
        let d = route(
            &["errsig:divzero".into()],
            &default_genes(),
            &HistoryAnalysis::default(),
            &RouterConfig::default(),
        );
        assert_eq!(d.category, Some(CycleIntent::Repair));
    }

    #[test]
    fn confidence_is_one_when_single_match() {
        let d = route(
            &["user_feature_request".into()],
            &default_genes(),
            &HistoryAnalysis::default(),
            &RouterConfig::default(),
        );
        assert!((d.confidence - 1.0).abs() < 1e-6);
    }

    #[test]
    fn alternatives_surfaced_on_tie_or_near_tie() {
        // Use signals that hit multiple genes to force alternatives to show.
        let d = route(
            &["error".into(), "user_feature_request".into()],
            &default_genes(),
            &HistoryAnalysis::default(),
            &RouterConfig::default(),
        );
        assert!(d.selected_gene.is_some());
        assert!(!d.alternatives.is_empty());
    }

    #[test]
    fn suppression_penalty_downranks_suppressed_signals() {
        let mut history = HistoryAnalysis::default();
        history.suppressed_signals.insert("error".into());
        history.suppressed_signals.insert("failed".into());

        // Repair would normally win on "error"+"failed". With both suppressed
        // and only one innovate signal, innovate should catch up or win.
        let cfg = RouterConfig {
            suppression_penalty: 0.0, // fully suppress
            ..Default::default()
        };
        let d = route(
            &[
                "error".into(),
                "failed".into(),
                "user_feature_request".into(),
            ],
            &default_genes(),
            &history,
            &cfg,
        );
        // With full suppression, only the non-suppressed innovate match counts.
        assert_eq!(d.category, Some(CycleIntent::Innovate));
    }

    #[test]
    fn tiebreak_is_deterministic_by_gene_id() {
        // Same call twice must give the same answer.
        let d1 = route(
            &["error".into()],
            &default_genes(),
            &HistoryAnalysis::default(),
            &RouterConfig::default(),
        );
        let d2 = route(
            &["error".into()],
            &default_genes(),
            &HistoryAnalysis::default(),
            &RouterConfig::default(),
        );
        assert_eq!(d1, d2);
    }
}
