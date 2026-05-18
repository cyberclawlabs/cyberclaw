//! Semantic memory tier for CyberClaw — curated, cross-session agent notes.
//!
//! # Overview
//!
//! Semantic memory is the 4th envelope section injected into the system prompt.
//! Unlike `working` (session-scoped), `episodic` (execution history), or
//! `procedural` (rule documents), semantic memory is a **curated knowledge
//! base** the agent maintains over time: "what I've learned about this
//! environment" and "what I know about the user".
//!
//! Ported from the Hermes agent reference design (`memories/MEMORY.md` +
//! `memories/USER.md`). Core-side contains only the trait and DTOs — all
//! file I/O and locking live in the owner connector
//! (`cyberclaw_connectors::local::memory`) to keep this crate dependency-free
//! (see §4 of `docs/architecture/idioms/EVOLUTION_IDIOMS.md`).
//!
//! # The frozen-snapshot pattern
//!
//! [`SemanticMemory::load_snapshot`] returns a [`SemanticSnapshot`] captured at
//! session start. The snapshot is what [`MemoryContextProvider`] injects into
//! the prompt. Mid-session mutations (`add` / `replace` / `remove`) update the
//! underlying store but **do not** change the snapshot — this keeps the system
//! prompt prefix stable across turns and preserves the prefix cache.
//!
//! # Character limits
//!
//! Both document kinds have model-independent character caps enforced on every
//! write. The defaults match the Hermes reference:
//!
//! - [`MemoryDocumentKind::AgentNotes`] — 2200 chars
//! - [`MemoryDocumentKind::UserProfile`] — 1375 chars
//!
//! [`MemoryContextProvider`]: crate::memory::provider::MemoryContextProvider

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Entry delimiter used between multi-line semantic entries.
///
/// Matches Hermes' `ENTRY_DELIMITER`. Stored in the trait module so both the
/// core-side DTOs and the connector-side file implementation agree on the
/// on-disk format.
pub const ENTRY_DELIMITER: &str = "\n§\n";

/// Default character limit for [`MemoryDocumentKind::AgentNotes`].
pub const DEFAULT_AGENT_NOTES_CHAR_LIMIT: usize = 2200;

/// Default character limit for [`MemoryDocumentKind::UserProfile`].
pub const DEFAULT_USER_PROFILE_CHAR_LIMIT: usize = 1375;

// ---------------------------------------------------------------------------
// Document kind
// ---------------------------------------------------------------------------

/// The two document kinds that make up a semantic memory store.
///
/// - `AgentNotes` — the agent's personal notes (environment facts, project
///   conventions, tool quirks, lessons learned). Maps to `MEMORY.md` in Hermes.
/// - `UserProfile` — what the agent knows about the user (preferences,
///   communication style, workflow habits). Maps to `USER.md` in Hermes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryDocumentKind {
    /// Agent's personal notes — maps to `MEMORY.md` in the Hermes reference.
    AgentNotes,
    /// Knowledge about the user — maps to `USER.md` in the Hermes reference.
    UserProfile,
}

impl MemoryDocumentKind {
    /// A short stable identifier suitable for tool arguments (`"agent_notes"`
    /// or `"user_profile"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentNotes => "agent_notes",
            Self::UserProfile => "user_profile",
        }
    }

    /// Default character limit for this kind.
    pub fn default_char_limit(self) -> usize {
        match self {
            Self::AgentNotes => DEFAULT_AGENT_NOTES_CHAR_LIMIT,
            Self::UserProfile => DEFAULT_USER_PROFILE_CHAR_LIMIT,
        }
    }
}

/// Unit struct returned by [`MemoryDocumentKind::from_str`] when the input
/// doesn't match any known kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unknown MemoryDocumentKind identifier")]
pub struct UnknownMemoryDocumentKind;

impl std::str::FromStr for MemoryDocumentKind {
    type Err = UnknownMemoryDocumentKind;

    /// Parse a kind from its string identifier. Case-insensitive. Accepts
    /// both the canonical names (`agent_notes`, `user_profile`) and the
    /// Hermes aliases (`memory`, `user`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "agent_notes" | "memory" => Ok(Self::AgentNotes),
            "user_profile" | "user" => Ok(Self::UserProfile),
            _ => Err(UnknownMemoryDocumentKind),
        }
    }
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// A single semantic memory entry — a chunk of curated text between two
/// `§` delimiters on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Which document this entry belongs to.
    pub kind: MemoryDocumentKind,
    /// The entry text. Never contains the delimiter itself.
    pub content: String,
}

/// A full semantic memory document — the ordered list of entries plus usage
/// metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryDocument {
    /// Which document this is.
    pub kind: MemoryDocumentKind,
    /// Ordered list of entries (insertion order preserved).
    pub entries: Vec<String>,
    /// Total character count of the document when rendered with
    /// [`ENTRY_DELIMITER`] between entries.
    pub char_count: usize,
    /// The active character limit applied to this document.
    pub char_limit: usize,
}

impl MemoryDocument {
    /// Build a document from a list of entries, computing `char_count`
    /// consistent with [`ENTRY_DELIMITER`] rendering.
    pub fn from_entries(kind: MemoryDocumentKind, entries: Vec<String>, char_limit: usize) -> Self {
        let char_count = if entries.is_empty() {
            0
        } else {
            entries.join(ENTRY_DELIMITER).len()
        };
        Self {
            kind,
            entries,
            char_count,
            char_limit,
        }
    }

    /// Render the document as a single string using [`ENTRY_DELIMITER`].
    ///
    /// Returns an empty string when there are no entries.
    pub fn render(&self) -> String {
        if self.entries.is_empty() {
            String::new()
        } else {
            self.entries.join(ENTRY_DELIMITER)
        }
    }
}

/// Frozen snapshot of the semantic memory state at session start.
///
/// This is what [`MemoryContextProvider`] injects into the system prompt. It
/// captures the exact rendered text of both documents at load time.
/// Mid-session mutations update the store but **not** the snapshot — so the
/// system prompt prefix stays stable and the prefix cache remains warm for
/// the entire session.
///
/// [`MemoryContextProvider`]: crate::memory::provider::MemoryContextProvider
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticSnapshot {
    /// Frozen rendered text for [`MemoryDocumentKind::AgentNotes`].
    pub agent_notes: String,
    /// Frozen rendered text for [`MemoryDocumentKind::UserProfile`].
    pub user_profile: String,
    /// RFC3339 timestamp of when this snapshot was captured.
    pub captured_at: String,
}

impl SemanticSnapshot {
    /// Returns `true` when both documents are empty.
    pub fn is_empty(&self) -> bool {
        self.agent_notes.is_empty() && self.user_profile.is_empty()
    }

    /// Return the rendered document for a given kind.
    pub fn document(&self, kind: MemoryDocumentKind) -> &str {
        match kind {
            MemoryDocumentKind::AgentNotes => &self.agent_notes,
            MemoryDocumentKind::UserProfile => &self.user_profile,
        }
    }
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Core trait for a semantic memory store.
///
/// Implementors must be `Send + Sync` so they can be shared across async
/// tasks. The concrete file-backed implementation lives in
/// `cyberclaw-connectors` (see `LocalSemanticMemoryConnector`).
///
/// All mutating operations return the full [`MemoryDocument`] after the
/// change so callers can surface updated usage information to the LLM without
/// a follow-up read.
#[async_trait]
pub trait SemanticMemory: Send + Sync {
    /// Load the current on-disk state and capture a frozen
    /// [`SemanticSnapshot`] for system-prompt injection.
    ///
    /// This is the only method that captures the snapshot — mutating methods
    /// (`add`, `replace`, `remove`) deliberately do **not** refresh it.
    async fn load_snapshot(&self) -> SemanticResult<SemanticSnapshot>;

    /// Return the current live state of a document (post-mutation view).
    async fn read_document(&self, kind: MemoryDocumentKind) -> SemanticResult<MemoryDocument>;

    /// Append a new entry. Rejects the write when:
    ///
    /// - content is empty after trimming;
    /// - content matches a prompt-injection or exfiltration pattern;
    /// - content contains an invisible unicode character;
    /// - the resulting document would exceed the character limit.
    ///
    /// Duplicate entries are silently ignored (matches Hermes behavior).
    async fn add(&self, kind: MemoryDocumentKind, content: &str) -> SemanticResult<MemoryDocument>;

    /// Replace the first entry containing `match_substring` with
    /// `new_content`. Returns an error when no entry matches or multiple
    /// distinct entries match.
    async fn replace(
        &self,
        kind: MemoryDocumentKind,
        match_substring: &str,
        new_content: &str,
    ) -> SemanticResult<MemoryDocument>;

    /// Remove the first entry containing `match_substring`. Returns an error
    /// when no entry matches or multiple distinct entries match.
    async fn remove(
        &self,
        kind: MemoryDocumentKind,
        match_substring: &str,
    ) -> SemanticResult<MemoryDocument>;
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors produced by [`SemanticMemory`] implementations.
#[derive(Debug, thiserror::Error)]
pub enum SemanticError {
    /// Content was empty after trimming.
    #[error("content is empty")]
    EmptyContent,

    /// Content was rejected by the safety scanner.
    #[error("content blocked by safety scan: {0}")]
    Blocked(String),

    /// Adding / replacing the entry would exceed the character limit.
    #[error("character limit exceeded: would use {would_use}/{limit} chars ({kind})")]
    LimitExceeded {
        kind: &'static str,
        would_use: usize,
        limit: usize,
    },

    /// No entry matched the given substring.
    #[error("no entry matched substring: {0:?}")]
    NoMatch(String),

    /// Multiple distinct entries matched the substring — caller must be
    /// more specific.
    #[error("multiple entries matched substring: {0:?} (be more specific)")]
    AmbiguousMatch(String),

    /// Underlying storage failure (file I/O, lock acquisition, etc.).
    #[error("storage failure: {0}")]
    Storage(String),
}

/// Convenience alias for [`SemanticMemory`] results.
pub type SemanticResult<T> = Result<T, SemanticError>;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // -----------------------------------------------------------------------
    // DTO serde roundtrips
    // -----------------------------------------------------------------------

    #[test]
    fn test_memory_document_kind_as_str_and_from_str() {
        use std::str::FromStr;

        assert_eq!(MemoryDocumentKind::AgentNotes.as_str(), "agent_notes");
        assert_eq!(MemoryDocumentKind::UserProfile.as_str(), "user_profile");

        assert_eq!(
            MemoryDocumentKind::from_str("agent_notes").unwrap(),
            MemoryDocumentKind::AgentNotes
        );
        assert_eq!(
            MemoryDocumentKind::from_str("memory").unwrap(),
            MemoryDocumentKind::AgentNotes,
            "hermes alias 'memory' must map to AgentNotes"
        );
        assert_eq!(
            MemoryDocumentKind::from_str("USER_PROFILE").unwrap(),
            MemoryDocumentKind::UserProfile,
            "parsing must be case-insensitive"
        );
        assert_eq!(
            MemoryDocumentKind::from_str("user").unwrap(),
            MemoryDocumentKind::UserProfile,
            "hermes alias 'user' must map to UserProfile"
        );
        assert!(MemoryDocumentKind::from_str("unknown").is_err());
    }

    #[test]
    fn test_memory_document_kind_default_limits() {
        assert_eq!(
            MemoryDocumentKind::AgentNotes.default_char_limit(),
            DEFAULT_AGENT_NOTES_CHAR_LIMIT
        );
        assert_eq!(
            MemoryDocumentKind::UserProfile.default_char_limit(),
            DEFAULT_USER_PROFILE_CHAR_LIMIT
        );
    }

    #[test]
    fn test_memory_entry_serde_roundtrip() {
        let entry = MemoryEntry {
            kind: MemoryDocumentKind::AgentNotes,
            content: "Project uses rustfmt and clippy -D warnings".to_string(),
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        let back: MemoryEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, entry);
        // Ensure the tag is lowercase_snake as configured.
        assert!(json.contains("agent_notes"));
    }

    #[test]
    fn test_memory_document_from_entries_char_count() {
        let doc = MemoryDocument::from_entries(
            MemoryDocumentKind::AgentNotes,
            vec!["abc".to_string(), "defgh".to_string()],
            100,
        );
        // "abc" (3) + delimiter "\n§\n" (5 bytes: \n, §(2 bytes), \n) + "defgh" (5) = 13
        assert_eq!(doc.char_count, "abc\n§\ndefgh".len());
        assert_eq!(doc.render(), "abc\n§\ndefgh");
    }

    #[test]
    fn test_memory_document_empty_render() {
        let doc = MemoryDocument::from_entries(MemoryDocumentKind::UserProfile, vec![], 100);
        assert_eq!(doc.char_count, 0);
        assert_eq!(doc.render(), "");
    }

    #[test]
    fn test_memory_document_serde_roundtrip() {
        let doc = MemoryDocument::from_entries(
            MemoryDocumentKind::UserProfile,
            vec![
                "Name: Alice".to_string(),
                "Prefers terse responses".to_string(),
            ],
            1375,
        );
        let json = serde_json::to_string(&doc).expect("serialize");
        let back: MemoryDocument = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, doc);
    }

    #[test]
    fn test_semantic_snapshot_serde_roundtrip_and_accessors() {
        let snap = SemanticSnapshot {
            agent_notes: "workspace uses cargo".to_string(),
            user_profile: "user prefers Rust".to_string(),
            captured_at: "2026-04-18T00:00:00Z".to_string(),
        };
        assert!(!snap.is_empty());
        assert_eq!(
            snap.document(MemoryDocumentKind::AgentNotes),
            "workspace uses cargo"
        );
        assert_eq!(
            snap.document(MemoryDocumentKind::UserProfile),
            "user prefers Rust"
        );

        let json = serde_json::to_string(&snap).expect("serialize");
        let back: SemanticSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, snap);
    }

    #[test]
    fn test_semantic_snapshot_default_is_empty() {
        let snap = SemanticSnapshot::default();
        assert!(snap.is_empty());
        assert_eq!(snap.document(MemoryDocumentKind::AgentNotes), "");
        assert_eq!(snap.document(MemoryDocumentKind::UserProfile), "");
    }

    #[test]
    fn test_entry_delimiter_constant() {
        // Freeze the on-disk format so connector and core stay in sync.
        assert_eq!(ENTRY_DELIMITER, "\n§\n");
    }

    // -----------------------------------------------------------------------
    // Trait mock (verifies trait is object-safe and usable)
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct MockSemanticMemory {
        agent: Arc<Mutex<Vec<String>>>,
        user: Arc<Mutex<Vec<String>>>,
    }

    impl MockSemanticMemory {
        fn store_for(&self, kind: MemoryDocumentKind) -> Arc<Mutex<Vec<String>>> {
            match kind {
                MemoryDocumentKind::AgentNotes => Arc::clone(&self.agent),
                MemoryDocumentKind::UserProfile => Arc::clone(&self.user),
            }
        }
    }

    #[async_trait]
    impl SemanticMemory for MockSemanticMemory {
        async fn load_snapshot(&self) -> SemanticResult<SemanticSnapshot> {
            let agent = self.agent.lock().unwrap().join(ENTRY_DELIMITER);
            let user = self.user.lock().unwrap().join(ENTRY_DELIMITER);
            Ok(SemanticSnapshot {
                agent_notes: agent,
                user_profile: user,
                captured_at: "mock".to_string(),
            })
        }

        async fn read_document(&self, kind: MemoryDocumentKind) -> SemanticResult<MemoryDocument> {
            let entries = self.store_for(kind).lock().unwrap().clone();
            Ok(MemoryDocument::from_entries(
                kind,
                entries,
                kind.default_char_limit(),
            ))
        }

        async fn add(
            &self,
            kind: MemoryDocumentKind,
            content: &str,
        ) -> SemanticResult<MemoryDocument> {
            let content = content.trim();
            if content.is_empty() {
                return Err(SemanticError::EmptyContent);
            }
            let store = self.store_for(kind);
            {
                let mut entries = store.lock().unwrap();
                if !entries.iter().any(|e| e == content) {
                    entries.push(content.to_string());
                }
            }
            self.read_document(kind).await
        }

        async fn replace(
            &self,
            kind: MemoryDocumentKind,
            match_substring: &str,
            new_content: &str,
        ) -> SemanticResult<MemoryDocument> {
            let store = self.store_for(kind);
            {
                let mut entries = store.lock().unwrap();
                let mut found = false;
                for e in entries.iter_mut() {
                    if e.contains(match_substring) {
                        *e = new_content.to_string();
                        found = true;
                        break;
                    }
                }
                if !found {
                    return Err(SemanticError::NoMatch(match_substring.to_string()));
                }
            }
            self.read_document(kind).await
        }

        async fn remove(
            &self,
            kind: MemoryDocumentKind,
            match_substring: &str,
        ) -> SemanticResult<MemoryDocument> {
            let store = self.store_for(kind);
            {
                let mut entries = store.lock().unwrap();
                let idx = entries.iter().position(|e| e.contains(match_substring));
                match idx {
                    Some(i) => {
                        entries.remove(i);
                    }
                    None => {
                        return Err(SemanticError::NoMatch(match_substring.to_string()));
                    }
                }
            }
            self.read_document(kind).await
        }
    }

    #[tokio::test]
    async fn test_mock_semantic_memory_roundtrip() {
        let mem: Box<dyn SemanticMemory> = Box::new(MockSemanticMemory::default());

        // initially empty
        let snap = mem.load_snapshot().await.unwrap();
        assert!(snap.is_empty());

        // add entries
        let doc = mem
            .add(MemoryDocumentKind::AgentNotes, "rustfmt is enforced")
            .await
            .unwrap();
        assert_eq!(doc.entries.len(), 1);
        assert_eq!(doc.char_count, "rustfmt is enforced".len());

        mem.add(MemoryDocumentKind::AgentNotes, "clippy -D warnings")
            .await
            .unwrap();
        mem.add(MemoryDocumentKind::UserProfile, "prefers Rust")
            .await
            .unwrap();

        // snapshot reflects current state when re-loaded (mock updates on each call)
        let snap = mem.load_snapshot().await.unwrap();
        assert!(snap.agent_notes.contains("rustfmt"));
        assert!(snap.agent_notes.contains("clippy"));
        assert!(snap.user_profile.contains("prefers Rust"));

        // replace
        let doc = mem
            .replace(
                MemoryDocumentKind::AgentNotes,
                "rustfmt",
                "fmt enforced via CI",
            )
            .await
            .unwrap();
        assert!(doc.entries.iter().any(|e| e == "fmt enforced via CI"));

        // remove
        let doc = mem
            .remove(MemoryDocumentKind::UserProfile, "prefers Rust")
            .await
            .unwrap();
        assert!(doc.entries.is_empty());

        // missing substring
        let err = mem
            .remove(MemoryDocumentKind::UserProfile, "nothing matches")
            .await
            .unwrap_err();
        assert!(matches!(err, SemanticError::NoMatch(_)));
    }

    #[tokio::test]
    async fn test_mock_semantic_memory_empty_content_rejected() {
        let mem: Box<dyn SemanticMemory> = Box::new(MockSemanticMemory::default());
        let err = mem
            .add(MemoryDocumentKind::AgentNotes, "   ")
            .await
            .unwrap_err();
        assert!(matches!(err, SemanticError::EmptyContent));
    }
}
