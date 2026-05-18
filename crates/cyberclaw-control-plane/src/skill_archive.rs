//! Skill Archive — variant population management with evolutionary selection.
//!
//! Borrowed from Meta FAIR HyperAgents (DGM-H) parent selection algorithms.
//! See `docs/implementation/reports/research-borrowing-analysis.md` SS9 (HA-H2, HA-H3).
//!
//! Lives in control-plane alongside `skill_evolution.rs` to avoid circular dependency
//! with skill-runtime.

use cyberclaw_core::ids::{ArtifactId, SkillId, VariantId};
use rand::distributions::WeightedIndex;
use rand::prelude::*;
use serde::{Deserialize, Serialize};

use crate::skill_evolution::CaseTrack;

// ============================================================================
// Skill Variant
// ============================================================================

/// A single Skill variant in the evolutionary archive.
///
/// Corresponds to one "generation" in the HyperAgents archive.
/// Each variant tracks its parent (lineage), evaluation score, and
/// how many child variants have been derived from it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillVariant {
    /// Unique variant identifier.
    pub variant_id: VariantId,
    /// Parent variant (None for root/seed variants).
    pub parent_variant_id: Option<VariantId>,
    /// The Skill this variant belongs to.
    pub skill_id: SkillId,
    /// Evaluation score (0.0 - 1.0).
    pub score: f32,
    /// Number of child variants derived from this one.
    pub child_count: u32,
    /// Which case track produced this variant.
    pub track: CaseTrack,
    /// Typed reference to the diff patch artifact (ArtifactKind::Patch).
    pub patch_artifact_id: Option<ArtifactId>,
}

// ============================================================================
// Selection Method
// ============================================================================

/// Parent selection strategy for choosing which variant to evolve next.
///
/// Borrowed from HyperAgents `utils/gl_utils.py:511-587`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectionMethod {
    /// Select the variant with the highest score.
    Best,
    /// Select the most recently added variant.
    Latest,
    /// Uniform random selection.
    Random,
    /// Probability proportional to sigmoid-normalized score.
    ScoreProp,
    /// Sigmoid-normalized score weighted by child count penalty.
    ///
    /// `weight = sigmoid((score - mid_point) * 5) * exp(-(child_count / 8)^3)`
    ///
    /// Balances exploitation (prefer high scores) with exploration
    /// (prefer variants that haven't been explored much).
    ScoreChildProp,
}

// ============================================================================
// Selection Algorithm
// ============================================================================

fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// Compute selection weights for `ScoreChildProp` method.
///
/// Algorithm from HyperAgents `score_child_prop`:
/// - Normalize scores via sigmoid centered on mean of top-3 scores
/// - Penalize variants with many children via exponential decay
pub fn score_child_prop_weights(variants: &[SkillVariant]) -> Vec<f64> {
    if variants.is_empty() {
        return vec![];
    }

    let scores: Vec<f64> = variants.iter().map(|v| v.score as f64).collect();

    // mid_point = mean of top 3 scores (HyperAgents original)
    let mut sorted = scores.clone();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let top_n: Vec<f64> = sorted.into_iter().take(3).collect();
    let mid_point = top_n.iter().sum::<f64>() / top_n.len() as f64;

    variants
        .iter()
        .map(|v| {
            let s = v.score as f64;
            let normalized = sigmoid((s - mid_point) * 5.0);
            let child_penalty = (-((v.child_count as f64 / 8.0).powi(3))).exp();
            normalized * child_penalty
        })
        .collect()
}

/// Compute selection weights for `ScoreProp` method.
fn score_prop_weights(variants: &[SkillVariant]) -> Vec<f64> {
    if variants.is_empty() {
        return vec![];
    }

    let scores: Vec<f64> = variants.iter().map(|v| v.score as f64).collect();
    let mut sorted = scores.clone();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let top_n: Vec<f64> = sorted.into_iter().take(3).collect();
    let mid_point = top_n.iter().sum::<f64>() / top_n.len() as f64;

    variants
        .iter()
        .map(|v| sigmoid((v.score as f64 - mid_point) * 5.0))
        .collect()
}

/// Select a parent variant from the given candidates.
///
/// Returns `None` if the candidates slice is empty.
pub fn select_parent(variants: &[SkillVariant], method: SelectionMethod) -> Option<usize> {
    if variants.is_empty() {
        return None;
    }

    match method {
        SelectionMethod::Best => {
            let idx = variants
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| {
                    a.score
                        .partial_cmp(&b.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| i);
            idx
        }
        SelectionMethod::Latest => Some(variants.len() - 1),
        SelectionMethod::Random => {
            let mut rng = thread_rng();
            Some(rng.gen_range(0..variants.len()))
        }
        SelectionMethod::ScoreProp => {
            let weights = score_prop_weights(variants);
            weighted_select(&weights)
        }
        SelectionMethod::ScoreChildProp => {
            let weights = score_child_prop_weights(variants);
            weighted_select(&weights)
        }
    }
}

fn weighted_select(weights: &[f64]) -> Option<usize> {
    if weights.is_empty() {
        return None;
    }
    // Ensure all weights are non-negative and at least one is positive
    let has_positive = weights.iter().any(|&w| w > 0.0);
    if !has_positive {
        // Fallback to uniform if all weights are zero
        let mut rng = thread_rng();
        return Some(rng.gen_range(0..weights.len()));
    }
    let safe_weights: Vec<f64> = weights.iter().map(|&w| w.max(0.0)).collect();
    let dist = WeightedIndex::new(&safe_weights).ok()?;
    let mut rng = thread_rng();
    Some(dist.sample(&mut rng))
}

// ============================================================================
// Skill Archive
// ============================================================================

/// Archive of Skill variants with lineage tracking.
///
/// Maintains the full evolutionary history of a Skill, supporting
/// parent selection for the next evolution cycle and lineage queries
/// for audit/provenance.
///
/// Inspired by HyperAgents JSONL archive format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillArchive {
    /// The Skill this archive tracks.
    pub skill_id: SkillId,
    /// All variants in generation order.
    pub variants: Vec<SkillVariant>,
}

impl SkillArchive {
    /// Create a new empty archive for a Skill.
    pub fn new(skill_id: SkillId) -> Self {
        Self {
            skill_id,
            variants: Vec::new(),
        }
    }

    /// Add a new variant to the archive.
    pub fn add_variant(&mut self, variant: SkillVariant) {
        // Increment parent's child count if applicable
        if let Some(ref parent_id) = variant.parent_variant_id {
            if let Some(parent) = self
                .variants
                .iter_mut()
                .find(|v| &v.variant_id == parent_id)
            {
                parent.child_count += 1;
            }
        }
        self.variants.push(variant);
    }

    /// Select a parent variant using the given method.
    pub fn select_parent(&self, method: SelectionMethod) -> Option<&SkillVariant> {
        select_parent(&self.variants, method).map(|idx| &self.variants[idx])
    }

    /// Get the variant with the highest score.
    pub fn best_variant(&self) -> Option<&SkillVariant> {
        self.variants.iter().max_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Get the lineage chain from a variant back to the root.
    ///
    /// Returns variants in order: [target, parent, grandparent, ..., root].
    pub fn lineage(&self, variant_id: &VariantId) -> Vec<&SkillVariant> {
        let mut chain = Vec::new();
        let mut current_id = Some(variant_id);

        while let Some(id) = current_id {
            if let Some(variant) = self.variants.iter().find(|v| &v.variant_id == id) {
                chain.push(variant);
                current_id = variant.parent_variant_id.as_ref();
            } else {
                break;
            }
        }

        chain
    }

    /// Number of variants in the archive.
    pub fn len(&self) -> usize {
        self.variants.len()
    }

    /// Whether the archive is empty.
    pub fn is_empty(&self) -> bool {
        self.variants.is_empty()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_variant(score: f32, child_count: u32) -> SkillVariant {
        SkillVariant {
            variant_id: VariantId::new(),
            parent_variant_id: None,
            skill_id: SkillId::new(),
            score,
            child_count,
            track: CaseTrack::Success,
            patch_artifact_id: None,
        }
    }

    fn make_variant_with_id(id: VariantId, parent: Option<VariantId>, score: f32) -> SkillVariant {
        SkillVariant {
            variant_id: id,
            parent_variant_id: parent,
            skill_id: SkillId::new(),
            score,
            child_count: 0,
            track: CaseTrack::Success,
            patch_artifact_id: None,
        }
    }

    // -- sigmoid --

    #[test]
    fn test_sigmoid_basic() {
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-10);
        assert!(sigmoid(100.0) > 0.999);
        assert!(sigmoid(-100.0) < 0.001);
    }

    // -- score_child_prop_weights --

    #[test]
    fn test_weights_empty() {
        assert!(score_child_prop_weights(&[]).is_empty());
    }

    #[test]
    fn test_weights_single() {
        let variants = vec![make_variant(0.8, 0)];
        let weights = score_child_prop_weights(&variants);
        assert_eq!(weights.len(), 1);
        assert!(weights[0] > 0.0);
    }

    #[test]
    fn test_weights_high_score_low_children_preferred() {
        // Variant A: high score, 0 children
        // Variant B: high score, 10 children
        let variants = vec![make_variant(0.9, 0), make_variant(0.9, 10)];
        let weights = score_child_prop_weights(&variants);
        // A should have higher weight (less explored)
        assert!(
            weights[0] > weights[1],
            "Expected w[0]={} > w[1]={} (low children preferred)",
            weights[0],
            weights[1]
        );
    }

    #[test]
    fn test_weights_equal_scores_children_differentiate() {
        let variants = vec![
            make_variant(0.5, 0),
            make_variant(0.5, 5),
            make_variant(0.5, 10),
        ];
        let weights = score_child_prop_weights(&variants);
        // Fewer children = higher weight
        assert!(weights[0] > weights[1]);
        assert!(weights[1] > weights[2]);
    }

    #[test]
    fn test_weights_score_matters() {
        // Both 0 children, different scores
        let variants = vec![make_variant(0.9, 0), make_variant(0.1, 0)];
        let weights = score_child_prop_weights(&variants);
        assert!(
            weights[0] > weights[1],
            "Higher score should have higher weight when children equal"
        );
    }

    #[test]
    fn test_weights_all_positive() {
        let variants = vec![
            make_variant(0.1, 0),
            make_variant(0.5, 3),
            make_variant(0.9, 7),
        ];
        let weights = score_child_prop_weights(&variants);
        for w in &weights {
            assert!(*w >= 0.0, "Weight should be non-negative: {}", w);
        }
    }

    // -- select_parent --

    #[test]
    fn test_select_parent_empty() {
        assert!(select_parent(&[], SelectionMethod::Best).is_none());
        assert!(select_parent(&[], SelectionMethod::ScoreChildProp).is_none());
    }

    #[test]
    fn test_select_parent_single() {
        let variants = vec![make_variant(0.5, 0)];
        assert_eq!(select_parent(&variants, SelectionMethod::Best), Some(0));
        assert_eq!(select_parent(&variants, SelectionMethod::Latest), Some(0));
    }

    #[test]
    fn test_select_parent_best() {
        let variants = vec![
            make_variant(0.3, 0),
            make_variant(0.9, 0),
            make_variant(0.6, 0),
        ];
        assert_eq!(select_parent(&variants, SelectionMethod::Best), Some(1));
    }

    #[test]
    fn test_select_parent_latest() {
        let variants = vec![
            make_variant(0.9, 0),
            make_variant(0.3, 0),
            make_variant(0.6, 0),
        ];
        assert_eq!(select_parent(&variants, SelectionMethod::Latest), Some(2));
    }

    #[test]
    fn test_select_parent_random_in_range() {
        let variants = vec![make_variant(0.5, 0), make_variant(0.5, 0)];
        for _ in 0..20 {
            let idx = select_parent(&variants, SelectionMethod::Random).unwrap();
            assert!(idx < variants.len());
        }
    }

    #[test]
    fn test_select_parent_score_child_prop_in_range() {
        let variants = vec![
            make_variant(0.9, 0),
            make_variant(0.5, 3),
            make_variant(0.1, 7),
        ];
        for _ in 0..20 {
            let idx = select_parent(&variants, SelectionMethod::ScoreChildProp).unwrap();
            assert!(idx < variants.len());
        }
    }

    // -- SkillArchive --

    #[test]
    fn test_archive_new_empty() {
        let archive = SkillArchive::new(SkillId::new());
        assert!(archive.is_empty());
        assert_eq!(archive.len(), 0);
        assert!(archive.best_variant().is_none());
    }

    #[test]
    fn test_archive_add_and_best() {
        let skill_id = SkillId::new();
        let mut archive = SkillArchive::new(skill_id);
        archive.add_variant(make_variant(0.3, 0));
        archive.add_variant(make_variant(0.9, 0));
        archive.add_variant(make_variant(0.6, 0));

        assert_eq!(archive.len(), 3);
        let best = archive.best_variant().unwrap();
        assert!((best.score - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn test_archive_add_increments_parent_child_count() {
        let skill_id = SkillId::new();
        let mut archive = SkillArchive::new(skill_id);

        let parent_id = VariantId::new();
        let parent = make_variant_with_id(parent_id.clone(), None, 0.8);
        archive.add_variant(parent);

        assert_eq!(archive.variants[0].child_count, 0);

        let child = make_variant_with_id(VariantId::new(), Some(parent_id.clone()), 0.7);
        archive.add_variant(child);

        assert_eq!(archive.variants[0].child_count, 1);

        let child2 = make_variant_with_id(VariantId::new(), Some(parent_id), 0.6);
        archive.add_variant(child2);

        assert_eq!(archive.variants[0].child_count, 2);
    }

    #[test]
    fn test_archive_lineage() {
        let skill_id = SkillId::new();
        let mut archive = SkillArchive::new(skill_id);

        let root_id = VariantId::new();
        let mid_id = VariantId::new();
        let leaf_id = VariantId::new();

        archive.add_variant(make_variant_with_id(root_id.clone(), None, 0.5));
        archive.add_variant(make_variant_with_id(
            mid_id.clone(),
            Some(root_id.clone()),
            0.7,
        ));
        archive.add_variant(make_variant_with_id(
            leaf_id.clone(),
            Some(mid_id.clone()),
            0.9,
        ));

        let lineage = archive.lineage(&leaf_id);
        assert_eq!(lineage.len(), 3);
        assert_eq!(lineage[0].variant_id, leaf_id);
        assert_eq!(lineage[1].variant_id, mid_id);
        assert_eq!(lineage[2].variant_id, root_id);
    }

    #[test]
    fn test_archive_lineage_root_only() {
        let skill_id = SkillId::new();
        let mut archive = SkillArchive::new(skill_id);

        let root_id = VariantId::new();
        archive.add_variant(make_variant_with_id(root_id.clone(), None, 0.5));

        let lineage = archive.lineage(&root_id);
        assert_eq!(lineage.len(), 1);
        assert_eq!(lineage[0].variant_id, root_id);
    }

    #[test]
    fn test_archive_lineage_unknown_id() {
        let archive = SkillArchive::new(SkillId::new());
        let unknown = VariantId::new();
        let lineage = archive.lineage(&unknown);
        assert!(lineage.is_empty());
    }

    #[test]
    fn test_archive_select_parent() {
        let skill_id = SkillId::new();
        let mut archive = SkillArchive::new(skill_id);
        archive.add_variant(make_variant(0.3, 0));
        archive.add_variant(make_variant(0.9, 0));

        let best = archive.select_parent(SelectionMethod::Best).unwrap();
        assert!((best.score - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn test_archive_serde_roundtrip() {
        let skill_id = SkillId::new();
        let mut archive = SkillArchive::new(skill_id);
        archive.add_variant(make_variant(0.8, 2));

        let json = serde_json::to_string(&archive).expect("serialize");
        let deser: SkillArchive = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.len(), 1);
        assert!((deser.variants[0].score - 0.8).abs() < f32::EPSILON);
        assert_eq!(deser.variants[0].child_count, 2);
    }

    // -- score_prop_weights --

    #[test]
    fn test_score_prop_weights_higher_score_higher_weight() {
        let variants = vec![make_variant(0.9, 0), make_variant(0.1, 0)];
        let weights = score_prop_weights(&variants);
        assert!(weights[0] > weights[1]);
    }
}
