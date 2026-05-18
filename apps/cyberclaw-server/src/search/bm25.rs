//! Keyword BM25 scoring for memory search (Sprint 20 P4 A).
//!
//! v1 simplification: no corpus-level IDF statistics — uniform doc frequency
//! assumed. Score is suitable for ranking within a single-tenant demo dataset.

const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;
/// Assumed average document length in tokens (constant for v1).
const AVG_DOC_LEN: f64 = 500.0;

/// Split `text` into lowercase alphanumeric tokens, discarding single-char tokens.
pub fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty() && s.len() > 1)
        .map(|s| s.to_string())
        .collect()
}

/// Compute a BM25-style score for `query_tokens` against `doc` text.
///
/// Returns 0.0 when no query token appears in the document.
pub fn bm25_score(query_tokens: &[String], doc: &str) -> f64 {
    let doc_tokens = tokenize(doc);
    let doc_len = doc_tokens.len() as f64;
    if doc_len == 0.0 {
        return 0.0;
    }

    let mut score = 0.0;
    for q in query_tokens {
        let tf = doc_tokens.iter().filter(|t| *t == q).count() as f64;
        if tf == 0.0 {
            continue;
        }
        let numer = tf * (BM25_K1 + 1.0);
        let denom = tf + BM25_K1 * (1.0 - BM25_B + BM25_B * doc_len / AVG_DOC_LEN);
        score += numer / denom;
    }
    score
}

/// Return the subset of `query_tokens` that appear in `doc`.
pub fn find_matches(query_tokens: &[String], doc: &str) -> Vec<String> {
    let doc_tokens = tokenize(doc);
    query_tokens
        .iter()
        .filter(|q| doc_tokens.contains(q))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_basic() {
        let tokens = tokenize("Hello, World!");
        assert_eq!(tokens, vec!["hello", "world"]);
    }

    #[test]
    fn test_tokenize_filters_single_char() {
        let tokens = tokenize("a bb ccc");
        assert_eq!(tokens, vec!["bb", "ccc"]);
    }

    #[test]
    fn test_bm25_scoring_nonzero() {
        let query = tokenize("foo bar");
        let doc = "foo bar baz foo";
        let score = bm25_score(&query, doc);
        assert!(score > 0.0, "expected positive score for matching doc");
    }

    #[test]
    fn test_bm25_scoring_zero_no_match() {
        let query = tokenize("xyz");
        let doc = "foo bar baz";
        let score = bm25_score(&query, doc);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_bm25_ranks_by_overlap() {
        let query = tokenize("foo bar");
        let high_doc = "foo bar foo bar foo";
        let low_doc = "foo something else";
        let high_score = bm25_score(&query, high_doc);
        let low_score = bm25_score(&query, low_doc);
        assert!(
            high_score > low_score,
            "doc with more query token overlap should score higher"
        );
    }

    #[test]
    fn test_find_matches_returns_intersect() {
        let query = tokenize("foo bar baz");
        let doc = "foo something bar";
        let matches = find_matches(&query, doc);
        assert!(matches.contains(&"foo".to_string()));
        assert!(matches.contains(&"bar".to_string()));
        assert!(!matches.contains(&"baz".to_string()));
    }
}
