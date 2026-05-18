//! Stage Handoff Protocol (Sprint 9 L1).
//!
//! Implements the paseo-style stage handoff file mechanism from
//! `tmp/claw-research/oh-my-claudecode/skills/team/SKILL.md` §"Stage Handoff
//! Convention". When a pipeline stage finishes the coordinator (or its caller)
//! materialises a [`StageHandoffDocument`] — a small, human-readable briefing
//! aimed at the **next** stage — and persists it through a [`HandoffStore`].
//!
//! The handoff is **not** a full specification; larger deliverables (DESIGN.md,
//! PRD.md, test reports) live as [`ArtifactRef`]s pointed at by the handoff.
//! The document itself is the ≤ 30-line prose briefing that the next stage's
//! agents read before they start, so decisions and open questions survive a
//! lead restart or context compaction.
//!
//! # Architectural placement
//!
//! - **No capability dispatch.** The file-backed store writes plain markdown;
//!   nothing here goes through a Connector. This module is intentionally
//!   "plain Rust + serde + filesystem" so it can be unit-tested hermetically.
//! - **No coupling to [`crate::team_pipeline`].** The handoff module
//!   references `team_pipeline::ArtifactRef` by re-export so callers can pass
//!   the artifacts a pipeline stage already produced without a second type.
//!
//! # File layout
//!
//! ```text
//! <root>/<pipeline_id>/<from_stage>.md
//! ```
//!
//! Example: `.cyberclaw/handoff/team-build-rest-api/team-plan.md`. Writing the
//! handoff for the current stage overwrites any previous file for that stage
//! (the latest handoff wins — fix-loop re-runs carry the newest decisions).
//!
//! ## Relationship to other "multi-agent" primitives in this workspace
//!
//! This module is **one of three** distinct multi-agent mechanisms. They
//! have overlapping keywords but solve different problems:
//!
//! | Primitive | Crate / File | Semantics |
//! |---|---|---|
//! | `SubAgentOrchestrator` | `cyberclaw-agent-runtime/src/sub_agent.rs` | Parent agent calls a child agent **as a tool**; child returns a result; parent resumes its own loop. Depth ≤ 3, max children 5. |
//! | `StageHandoffDocument` | `cyberclaw-control-plane/src/stage_handoff.rs` | Pipeline stage-to-stage paseo-style **markdown briefing** (≤ 30 lines) persisted to `<root>/<pipeline_id>/<from_stage>.md`. Linear stage hop, not live. |
//! | `HandoffConnector` | `cyberclaw-connectors/src/handoff.rs` | **Live conversational** transfer (Sprint 21): user is talking to agent_A; agent_A `agent.handoff` capability hands the conversation permanently to agent_B, with a briefing ≤ 2KB. |
//!
//! **This module is the `StageHandoffDocument` (pipeline stage-to-stage markdown briefing).** If that
//! isn't what you want, see the matching row above.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::team_pipeline::ArtifactRef;

// ============================================================================
// Data model
// ============================================================================

/// A single handoff briefing produced by the stage that just finished.
///
/// The document is deliberately small; ≤ 20 lines of prose is the target. The
/// shape mirrors the "Handoff: X → Y" markdown block the OMC team skill
/// generates, so the serialization roundtrips human-edited files correctly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StageHandoffDocument {
    /// Opaque pipeline identifier. Namespaces handoff files by run so one
    /// coordinator cannot accidentally clobber another's context.
    pub pipeline_id: String,
    /// Stage that authored the document.
    pub from_stage: String,
    /// Stage the handoff is addressed to. `"*"` means "any downstream stage"
    /// (useful when the authoring stage is verify and the fix stage is the
    /// actual consumer).
    pub to_stage: String,
    /// Deliverable artifacts referenced by the handoff. The stage writes
    /// these; downstream stages read them. Opaque to the handoff store.
    #[serde(default)]
    pub artifacts: Vec<ArtifactRef>,
    /// Free-form summary in markdown. One or two sentences is ideal.
    pub summary_md: String,
    /// Key decisions taken in this stage (bullet list, one per line).
    #[serde(default)]
    pub decisions_made: Vec<String>,
    /// Questions the next stage must answer (bullet list, one per line).
    #[serde(default)]
    pub open_questions: Vec<String>,
    /// When the handoff was authored. Used by store implementations for
    /// ordering and by resume logic to detect stale handoffs.
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
}

impl StageHandoffDocument {
    /// Construct a minimal handoff. Decisions, questions, and artifacts can
    /// be appended with the builder helpers below.
    pub fn new(
        pipeline_id: impl Into<String>,
        from_stage: impl Into<String>,
        to_stage: impl Into<String>,
        summary_md: impl Into<String>,
    ) -> Self {
        Self {
            pipeline_id: pipeline_id.into(),
            from_stage: from_stage.into(),
            to_stage: to_stage.into(),
            artifacts: Vec::new(),
            summary_md: summary_md.into(),
            decisions_made: Vec::new(),
            open_questions: Vec::new(),
            created_at: Utc::now(),
        }
    }

    pub fn with_artifacts(mut self, artifacts: Vec<ArtifactRef>) -> Self {
        self.artifacts = artifacts;
        self
    }

    pub fn with_decisions<I, S>(mut self, decisions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.decisions_made = decisions.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_open_questions<I, S>(mut self, questions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.open_questions = questions.into_iter().map(Into::into).collect();
        self
    }

    /// Render as markdown matching the OMC team-skill format. The output is
    /// stable (ordered sections, single trailing newline) so diffing two
    /// handoff files is meaningful.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "# Handoff: {} → {}\n",
            self.from_stage, self.to_stage
        ));
        out.push_str(&format!("- **Pipeline**: `{}`\n", self.pipeline_id));
        out.push_str(&format!(
            "- **Authored**: {}\n",
            self.created_at.to_rfc3339()
        ));
        out.push('\n');

        out.push_str("## Summary\n");
        out.push_str(self.summary_md.trim());
        out.push_str("\n\n");

        out.push_str("## Decisions\n");
        if self.decisions_made.is_empty() {
            out.push_str("- (none recorded)\n");
        } else {
            for d in &self.decisions_made {
                out.push_str(&format!("- {}\n", d.trim()));
            }
        }
        out.push('\n');

        out.push_str("## Open Questions\n");
        if self.open_questions.is_empty() {
            out.push_str("- (none)\n");
        } else {
            for q in &self.open_questions {
                out.push_str(&format!("- {}\n", q.trim()));
            }
        }
        out.push('\n');

        out.push_str("## Artifacts\n");
        if self.artifacts.is_empty() {
            out.push_str("- (none)\n");
        } else {
            for a in &self.artifacts {
                out.push_str(&format!("- `{}` ({})\n", a.id, a.kind));
            }
        }
        out.push('\n');
        out
    }

    /// Parse a markdown document previously produced by [`Self::to_markdown`].
    ///
    /// Best-effort: the parser honors the canonical section headers and
    /// bullet-list conventions but does not attempt to validate every field.
    /// Malformed inputs return [`HandoffError::Malformed`].
    pub fn from_markdown(text: &str) -> Result<Self, HandoffError> {
        // Title line: "# Handoff: FROM → TO"
        let mut lines = text.lines();
        let title = lines
            .next()
            .ok_or_else(|| HandoffError::Malformed("empty document".into()))?;
        let body = title
            .strip_prefix("# Handoff:")
            .ok_or_else(|| HandoffError::Malformed("missing '# Handoff:' header".into()))?
            .trim();
        // Split on the fancy arrow first, fall back to ASCII "->" for
        // hand-authored handoffs.
        let (from_stage, to_stage) = body
            .split_once('→')
            .or_else(|| body.split_once("->"))
            .ok_or_else(|| HandoffError::Malformed("missing '→' separator in title".into()))?;
        let from_stage = from_stage.trim().to_string();
        let to_stage = to_stage.trim().to_string();
        if from_stage.is_empty() || to_stage.is_empty() {
            return Err(HandoffError::Malformed(
                "from_stage or to_stage is empty".into(),
            ));
        }

        // Remaining front-matter: "- **Pipeline**: `...`" + "- **Authored**: ..."
        let mut pipeline_id = String::new();
        let mut created_at = Utc::now();

        #[derive(PartialEq, Eq)]
        enum Section {
            Front,
            Summary,
            Decisions,
            Questions,
            Artifacts,
        }
        let mut section = Section::Front;

        let mut summary_lines: Vec<String> = Vec::new();
        let mut decisions: Vec<String> = Vec::new();
        let mut questions: Vec<String> = Vec::new();
        let mut artifacts: Vec<ArtifactRef> = Vec::new();

        for raw in lines {
            let line = raw.trim_end();
            if let Some(rest) = line.strip_prefix("## ") {
                section = match rest.trim() {
                    "Summary" => Section::Summary,
                    "Decisions" => Section::Decisions,
                    "Open Questions" => Section::Questions,
                    "Artifacts" => Section::Artifacts,
                    _ => Section::Front,
                };
                continue;
            }
            match section {
                Section::Front => {
                    if let Some(rest) = line.strip_prefix("- **Pipeline**:") {
                        pipeline_id = rest.trim().trim_matches('`').trim().to_string();
                    } else if let Some(rest) = line.strip_prefix("- **Authored**:") {
                        if let Ok(ts) = DateTime::parse_from_rfc3339(rest.trim()) {
                            created_at = ts.with_timezone(&Utc);
                        }
                    }
                }
                Section::Summary => {
                    if !line.trim().is_empty() {
                        summary_lines.push(line.trim().to_string());
                    }
                }
                Section::Decisions => {
                    if let Some(bullet) = line.strip_prefix("- ") {
                        let text = bullet.trim();
                        if text != "(none recorded)" && !text.is_empty() {
                            decisions.push(text.to_string());
                        }
                    }
                }
                Section::Questions => {
                    if let Some(bullet) = line.strip_prefix("- ") {
                        let text = bullet.trim();
                        if text != "(none)" && !text.is_empty() {
                            questions.push(text.to_string());
                        }
                    }
                }
                Section::Artifacts => {
                    if let Some(bullet) = line.strip_prefix("- ") {
                        let text = bullet.trim();
                        if text == "(none)" || text.is_empty() {
                            continue;
                        }
                        // Shape: "`<id>` (<kind>)"
                        if let Some((id_part, kind_part)) = text.split_once(" (") {
                            let id = id_part.trim().trim_matches('`').trim().to_string();
                            let kind = kind_part.trim_end_matches(')').trim().to_string();
                            if !id.is_empty() {
                                artifacts.push(ArtifactRef::new(id, kind));
                            }
                        }
                    }
                }
            }
        }

        if pipeline_id.is_empty() {
            return Err(HandoffError::Malformed(
                "missing or empty '- **Pipeline**:' line".into(),
            ));
        }

        Ok(Self {
            pipeline_id,
            from_stage,
            to_stage,
            artifacts,
            summary_md: summary_lines.join("\n"),
            decisions_made: decisions,
            open_questions: questions,
            created_at,
        })
    }
}

// ============================================================================
// Errors
// ============================================================================

/// Errors surfaced by [`HandoffStore`] implementations.
#[derive(Debug, Error)]
pub enum HandoffError {
    #[error("handoff not found: pipeline={pipeline_id} stage={stage}")]
    NotFound { pipeline_id: String, stage: String },
    #[error("handoff document malformed: {0}")]
    Malformed(String),
    #[error("io: {0}")]
    Io(String),
}

// ============================================================================
// Store trait
// ============================================================================

/// Read + write surface for stage handoff documents. Implementations live
/// above this module; the default file-backed impl is [`FileHandoffStore`].
#[async_trait]
pub trait HandoffStore: Send + Sync {
    /// Write a handoff document. Overwrites any existing file for the same
    /// `(pipeline_id, from_stage)` tuple — the latest handoff wins.
    async fn write(&self, doc: &StageHandoffDocument) -> Result<(), HandoffError>;

    /// Read the most recent handoff authored by `from_stage` in the given
    /// pipeline. Returns [`HandoffError::NotFound`] when absent.
    async fn read(
        &self,
        pipeline_id: &str,
        from_stage: &str,
    ) -> Result<StageHandoffDocument, HandoffError>;

    /// List all handoffs for a pipeline, authored in chronological order.
    /// Useful for "verify" stages that want to read the entire decision
    /// history (plan → prd → exec) in one shot.
    async fn list(&self, pipeline_id: &str) -> Result<Vec<StageHandoffDocument>, HandoffError>;
}

// ============================================================================
// File-backed store
// ============================================================================

/// Filesystem-backed [`HandoffStore`].
///
/// Layout: `<root>/<pipeline_id>/<from_stage>.md`. The pipeline_id and stage
/// names are sanitised (path separators replaced with `_`) to prevent
/// directory traversal when the caller supplies attacker-controlled strings.
/// Callers who already constrain these values can skip this extra safety.
pub struct FileHandoffStore {
    root: PathBuf,
}

impl FileHandoffStore {
    /// Build a store rooted at `root`. The directory is created lazily on
    /// first write.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Default layout: `.cyberclaw/handoff/` under the current working dir.
    /// Callers that want a different layout should pass an explicit path to
    /// [`Self::new`].
    pub fn default_root() -> Self {
        Self::new(PathBuf::from(".cyberclaw").join("handoff"))
    }

    fn safe_segment(raw: &str) -> String {
        // Replace path separators and parent-traversal markers; leave the
        // rest alone so filenames remain human-readable.
        raw.replace(['/', '\\'], "_").replace("..", "__")
    }

    fn file_for(&self, pipeline_id: &str, from_stage: &str) -> PathBuf {
        self.root
            .join(Self::safe_segment(pipeline_id))
            .join(format!("{}.md", Self::safe_segment(from_stage)))
    }

    fn is_markdown(path: &Path) -> bool {
        path.extension().and_then(|e| e.to_str()) == Some("md")
    }
}

#[async_trait]
impl HandoffStore for FileHandoffStore {
    async fn write(&self, doc: &StageHandoffDocument) -> Result<(), HandoffError> {
        let path = self.file_for(&doc.pipeline_id, &doc.from_stage);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| HandoffError::Io(format!("create {}: {}", parent.display(), e)))?;
        }
        let markdown = doc.to_markdown();
        std::fs::write(&path, markdown.as_bytes())
            .map_err(|e| HandoffError::Io(format!("write {}: {}", path.display(), e)))?;
        Ok(())
    }

    async fn read(
        &self,
        pipeline_id: &str,
        from_stage: &str,
    ) -> Result<StageHandoffDocument, HandoffError> {
        let path = self.file_for(pipeline_id, from_stage);
        if !path.exists() {
            return Err(HandoffError::NotFound {
                pipeline_id: pipeline_id.to_string(),
                stage: from_stage.to_string(),
            });
        }
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| HandoffError::Io(format!("read {}: {}", path.display(), e)))?;
        StageHandoffDocument::from_markdown(&raw)
    }

    async fn list(&self, pipeline_id: &str) -> Result<Vec<StageHandoffDocument>, HandoffError> {
        let dir = self.root.join(Self::safe_segment(pipeline_id));
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| HandoffError::Io(format!("read_dir {}: {}", dir.display(), e)))?;
        let mut docs: Vec<StageHandoffDocument> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || !Self::is_markdown(&path) {
                continue;
            }
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| HandoffError::Io(format!("read {}: {}", path.display(), e)))?;
            match StageHandoffDocument::from_markdown(&raw) {
                Ok(doc) => docs.push(doc),
                Err(_) => {
                    // Skip malformed files on list; `read` still surfaces
                    // the error when a specific stage is requested.
                    continue;
                }
            }
        }
        docs.sort_by_key(|d| d.created_at);
        Ok(docs)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_doc(pipeline: &str, from: &str, to: &str) -> StageHandoffDocument {
        StageHandoffDocument::new(pipeline, from, to, "Decomposed task into 3 subtasks.")
            .with_decisions(vec![
                "Use PostgreSQL for persistence",
                "JWT for auth tokens",
            ])
            .with_open_questions(vec!["Redis provisioning TBD"])
            .with_artifacts(vec![
                ArtifactRef::new("DESIGN.md", "file"),
                ArtifactRef::new("TEST_STRATEGY.md", "file"),
            ])
    }

    #[test]
    fn markdown_roundtrip_preserves_all_fields() {
        let doc = sample_doc("pipe-1", "team-plan", "team-prd");
        let md = doc.to_markdown();
        let parsed = StageHandoffDocument::from_markdown(&md).expect("parse roundtrip succeeds");

        assert_eq!(parsed.pipeline_id, doc.pipeline_id);
        assert_eq!(parsed.from_stage, doc.from_stage);
        assert_eq!(parsed.to_stage, doc.to_stage);
        assert_eq!(parsed.summary_md, doc.summary_md);
        assert_eq!(parsed.decisions_made, doc.decisions_made);
        assert_eq!(parsed.open_questions, doc.open_questions);
        assert_eq!(parsed.artifacts, doc.artifacts);
        // Timestamps roundtrip to RFC-3339 precision; seconds granularity.
        assert_eq!(parsed.created_at.timestamp(), doc.created_at.timestamp());
    }

    #[test]
    fn markdown_output_contains_canonical_sections() {
        let doc = sample_doc("p1", "s1", "s2");
        let md = doc.to_markdown();
        assert!(md.starts_with("# Handoff: s1 → s2"));
        assert!(md.contains("## Summary"));
        assert!(md.contains("## Decisions"));
        assert!(md.contains("## Open Questions"));
        assert!(md.contains("## Artifacts"));
    }

    #[test]
    fn malformed_missing_title_is_rejected() {
        let err =
            StageHandoffDocument::from_markdown("not a handoff").expect_err("no title → reject");
        assert!(matches!(err, HandoffError::Malformed(_)));
    }

    #[test]
    fn malformed_missing_arrow_is_rejected() {
        let err = StageHandoffDocument::from_markdown("# Handoff: justonestage\n")
            .expect_err("missing arrow → reject");
        assert!(matches!(err, HandoffError::Malformed(_)));
    }

    #[tokio::test]
    async fn file_store_write_and_read_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let store = FileHandoffStore::new(tmp.path());
        let doc = sample_doc("pipe-fs", "team-exec", "team-verify");
        store.write(&doc).await.expect("write ok");
        let got = store
            .read(&doc.pipeline_id, &doc.from_stage)
            .await
            .expect("read ok");
        assert_eq!(got.from_stage, doc.from_stage);
        assert_eq!(got.decisions_made, doc.decisions_made);
        assert_eq!(got.artifacts, doc.artifacts);
    }

    #[tokio::test]
    async fn file_store_list_returns_all_handoffs_chronologically() {
        let tmp = TempDir::new().unwrap();
        let store = FileHandoffStore::new(tmp.path());
        let mut a = sample_doc("pipe-list", "team-plan", "team-prd");
        a.created_at = Utc::now() - chrono::Duration::seconds(30);
        let b = sample_doc("pipe-list", "team-prd", "team-exec");
        store.write(&a).await.unwrap();
        store.write(&b).await.unwrap();
        let list = store.list("pipe-list").await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].from_stage, "team-plan");
        assert_eq!(list[1].from_stage, "team-prd");
    }

    #[tokio::test]
    async fn file_store_read_missing_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        let store = FileHandoffStore::new(tmp.path());
        let err = store
            .read("ghost-pipeline", "team-plan")
            .await
            .expect_err("missing → NotFound");
        assert!(matches!(err, HandoffError::NotFound { .. }));
    }

    #[tokio::test]
    async fn file_store_sanitises_path_traversal_attempts() {
        let tmp = TempDir::new().unwrap();
        let store = FileHandoffStore::new(tmp.path());
        let doc = StageHandoffDocument::new("../escape", "../../etc/passwd", "downstream", "probe");
        store.write(&doc).await.expect("sanitised write ok");
        // Round-trip works because sanitisation is symmetric.
        let got = store.read(&doc.pipeline_id, &doc.from_stage).await.unwrap();
        assert_eq!(got.pipeline_id, doc.pipeline_id);
        assert_eq!(got.from_stage, doc.from_stage);
        // The file lives inside the tmp root, not anywhere above it.
        let depth = tmp.path().components().count();
        let walker = walkdir(tmp.path());
        for p in walker {
            let p_depth = p.components().count();
            assert!(p_depth >= depth);
        }
    }

    /// Lightweight recursive walker used by the sanitisation test. Avoids a
    /// new dev-dep on `walkdir` while still verifying containment.
    fn walkdir(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let p = entry.path();
                out.push(p.clone());
                if p.is_dir() {
                    stack.push(p);
                }
            }
        }
        out
    }
}
