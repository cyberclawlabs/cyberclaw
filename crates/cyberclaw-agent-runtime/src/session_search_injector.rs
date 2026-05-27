//! Session search injection for the agentic loop (v1.7 emergence wiring).
//!
//! Pairs with `constitution::IRON_LAW_8` to give every chat path a way to
//! pull prior-session context into the LLM's working memory without each
//! handler having to wire FTS5 itself.
//!
//! Default posture is **off** (`SessionSearchInjector::disabled()`) —
//! recall is LLM-driven via IRON LAW 8's `memory_search` tool call.
//! Operators who want passive pre-injection (e.g. always show the user's
//! top-3 relevant prior turns as system context) opt in with
//! `SessionSearchInjector::with_provider(...)`.
//!
//! # Why opt-in
//!
//! Always-on injection blows out context windows and surfaces noise; hm and
//! Claude Code both default to LLM-driven recall for the same reason.
//! Keeping this disabled by default mirrors that contract — see
//! `docs/research/agent-emergence-design-2026-05-27.md` §5 (举一反三 = skill
//! + memory + session_search 三件套, LLM-pulled).

use std::sync::Arc;

use cyberclaw_core::ids::SessionId;

// ---------------------------------------------------------------------------
// Provider trait — bridge to whatever FTS5 / vector store is wired
// ---------------------------------------------------------------------------

/// Read-only surface for "find me the top-k snippets that look relevant to
/// the current user query, across sessions belonging to this agent".
///
/// Implementations should be cheap (single SQLite FTS5 query is fine);
/// `inject_context` callers apply their own outer timeout if needed.
pub trait SessionSearchProvider: Send + Sync {
    /// Search past sessions for snippets relevant to `query`. Caller's
    /// `agent_id` and the *current* `session_id` may scope the search; the
    /// provider should still return cross-session hits (that's the whole
    /// point) but may de-prioritize the current session to avoid echoing
    /// the immediate conversation back.
    fn search(
        &self,
        agent_id: Option<&str>,
        current_session: Option<&SessionId>,
        query: &str,
        top_k: usize,
    ) -> Vec<SessionSearchHit>;
}

/// A single result row from [`SessionSearchProvider::search`].
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSearchHit {
    /// Origin session ID — useful for the LLM to cite ("从 session X").
    pub session_id: String,
    /// The matching snippet text. Caller may pre-trim to a reasonable size.
    pub snippet: String,
    /// FTS5 / BM25 relevance score (higher = more relevant). Used only for
    /// display; ordering is the provider's responsibility.
    pub score: f32,
}

// ---------------------------------------------------------------------------
// Injector
// ---------------------------------------------------------------------------

/// Wiring point for passive cross-session recall.
///
/// When enabled, [`SessionSearchInjector::inject_context`] prepends a small
/// "Relevant context from past sessions" block to the system prompt so the
/// LLM sees prior context without an explicit tool call. When disabled
/// (default), this struct is a no-op and recall happens via the LLM
/// calling `memory_search` per IRON LAW 8.
#[derive(Clone)]
pub struct SessionSearchInjector {
    provider: Option<Arc<dyn SessionSearchProvider>>,
    top_k: usize,
    max_snippet_chars: usize,
}

impl Default for SessionSearchInjector {
    fn default() -> Self {
        Self::disabled()
    }
}

impl SessionSearchInjector {
    /// Disabled injector — recall is fully LLM-driven via IRON LAW 8.
    pub fn disabled() -> Self {
        Self {
            provider: None,
            top_k: 3,
            max_snippet_chars: 280,
        }
    }

    /// Enable passive injection backed by `provider`.
    pub fn with_provider(provider: Arc<dyn SessionSearchProvider>) -> Self {
        Self {
            provider: Some(provider),
            top_k: 3,
            max_snippet_chars: 280,
        }
    }

    pub fn with_top_k(mut self, k: usize) -> Self {
        self.top_k = k.max(1);
        self
    }

    pub fn with_max_snippet_chars(mut self, n: usize) -> Self {
        self.max_snippet_chars = n.max(40);
        self
    }

    /// Returns true if this injector will actually fetch anything.
    pub fn is_enabled(&self) -> bool {
        self.provider.is_some()
    }

    /// Build the prefix block to prepend to a system prompt. Returns
    /// `None` when disabled or when no relevant hits are found — callers
    /// should fall back to their original prompt unchanged.
    pub fn build_context_block(
        &self,
        agent_id: Option<&str>,
        current_session: Option<&SessionId>,
        query: &str,
    ) -> Option<String> {
        let provider = self.provider.as_ref()?;
        let hits = provider.search(agent_id, current_session, query, self.top_k);
        if hits.is_empty() {
            return None;
        }
        let mut out = String::from("<past_sessions_context>\n");
        out.push_str("Relevant excerpts from your prior sessions (top-");
        out.push_str(&hits.len().to_string());
        out.push_str(", LLM may use as background; do NOT cite if irrelevant):\n");
        for hit in &hits {
            let snippet = truncate_snippet(&hit.snippet, self.max_snippet_chars);
            out.push_str(&format!(
                "- [session {} score={:.2}] {}\n",
                short_session(&hit.session_id),
                hit.score,
                snippet
            ));
        }
        out.push_str("</past_sessions_context>\n\n");
        Some(out)
    }

    /// Convenience: prepend the context block in-place onto an existing
    /// system prompt. No-op when disabled or when no hits found.
    pub fn inject_context(
        &self,
        system_prompt: &mut String,
        agent_id: Option<&str>,
        current_session: Option<&SessionId>,
        query: &str,
    ) {
        if let Some(block) = self.build_context_block(agent_id, current_session, query) {
            system_prompt.insert_str(0, &block);
        }
    }
}

fn truncate_snippet(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

fn short_session(id: &str) -> String {
    let char_count = id.chars().count();
    if char_count <= 12 {
        id.to_string()
    } else {
        let prefix: String = id.chars().take(12).collect();
        format!("{prefix}…")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedProvider(Vec<SessionSearchHit>);
    impl SessionSearchProvider for FixedProvider {
        fn search(
            &self,
            _agent_id: Option<&str>,
            _current_session: Option<&SessionId>,
            _query: &str,
            top_k: usize,
        ) -> Vec<SessionSearchHit> {
            self.0.iter().take(top_k).cloned().collect()
        }
    }

    #[test]
    fn disabled_is_noop() {
        let inj = SessionSearchInjector::disabled();
        let mut prompt = String::from("base prompt");
        inj.inject_context(&mut prompt, None, None, "anything");
        assert_eq!(prompt, "base prompt");
        assert!(!inj.is_enabled());
    }

    #[test]
    fn no_hits_skips_block() {
        let inj = SessionSearchInjector::with_provider(Arc::new(FixedProvider(vec![])));
        let mut prompt = String::from("base prompt");
        inj.inject_context(&mut prompt, None, None, "anything");
        assert_eq!(prompt, "base prompt");
    }

    #[test]
    fn hits_prepend_context_block() {
        let inj = SessionSearchInjector::with_provider(Arc::new(FixedProvider(vec![
            SessionSearchHit {
                session_id: "session-abc-123-xyz".to_string(),
                snippet: "user said unicornium was the rare term".to_string(),
                score: 0.85,
            },
            SessionSearchHit {
                session_id: "session-def-456".to_string(),
                snippet: "earlier we agreed to use blue ocean strategy".to_string(),
                score: 0.71,
            },
        ])));
        let mut prompt = String::from("base prompt");
        inj.inject_context(&mut prompt, None, None, "unicornium");
        assert!(prompt.starts_with("<past_sessions_context>"));
        assert!(prompt.contains("unicornium"));
        assert!(prompt.contains("base prompt"));
        assert!(prompt.contains("session-abc-"));
    }

    #[test]
    fn truncates_long_snippet() {
        let long = "x".repeat(500);
        let inj = SessionSearchInjector::with_provider(Arc::new(FixedProvider(vec![
            SessionSearchHit {
                session_id: "s1".to_string(),
                snippet: long,
                score: 1.0,
            },
        ])))
        .with_max_snippet_chars(40);
        let block = inj.build_context_block(None, None, "q").unwrap();
        // Snippet should be truncated to 40 'x' + '…'. The block has fixed
        // framing text on top of that, which may add a handful of unrelated
        // 'x' chars (e.g. "excerpts"). Just assert the snippet itself does
        // not bring all 500 'x's through.
        let xs: Vec<_> = block.match_indices('x').collect();
        assert!(
            xs.len() < 100,
            "expected truncated (<100 xs), got {} — snippet survived",
            xs.len()
        );
        assert!(block.contains('…'));
    }
}
