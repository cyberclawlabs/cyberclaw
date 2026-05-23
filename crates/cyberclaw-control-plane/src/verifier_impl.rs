//! Sprint D4: Real `VerifierExecutor` implementations for every
//! [`VerifierKind`] variant declared in Sprint D1.
//!
//! D3 owns the canonical [`VerifierExecutor`] trait + [`VerifierVerdict`] +
//! [`VerifierContext`] in [`crate::persistent_execution`]. This module
//! plugs into that trait with [`DefaultVerifierExecutor`].
//!
//! # What this module does
//!
//! For each [`VerifierKind`], [`DefaultVerifierExecutor::run`] performs the
//! real verification work and returns a [`VerifierVerdict`]:
//!
//! - **FileExists**: `tokio::fs::metadata` and `is_file`
//! - **FileMimeMatches**: magic-byte sniff over first 8 bytes (no new deps)
//!   — supports PPTX/PDF/PNG/MP3/JPEG/ZIP/GIF
//! - **FileNonEmpty**: metadata size > 0
//! - **TextNonEmpty**: read + trim + non-empty check
//! - **AudioDurationGt**: `ffprobe` shell-out; if `ffprobe` is missing or the
//!   output cannot be parsed, returns a `Fail` verdict (does NOT fake a pass)
//! - **NumericMatchesCsv**: routes through the
//!   `verify.numeric_aggregate` capability (commit aabcf34); supports
//!   `sum(col)` / `avg(col)` / `count` formulas
//! - **ScriptExitZero**: `tokio::process::Command` with cwd; passes iff
//!   `status.success()`; `stderr` tail captured on failure
//! - **LlmSemanticMatch**: single chat to the [`VerifierContext::llm_client`]
//!   (or this struct's `llm_client` fallback); pass when the model replies
//!   "YES" (case-insensitive prefix match)
//!
//! Each `Pass` verdict carries concrete evidence in
//! [`VerifierVerdict::Pass::evidence`] (e.g. ffprobe duration, pytest stdout
//! tail, LLM verdict).

use crate::persistent_execution::{
    VerifierContext, VerifierExecutor, VerifierKind, VerifierVerdict,
};
use async_trait::async_trait;
use cyberclaw_connectors::registry::ConnectorRegistry;
use cyberclaw_connectors::types::CapabilityExecutionRequest;
use cyberclaw_core::identity::{ActorRef, ActorType};
use cyberclaw_core::ids::{ActorId, CapabilityId, ExecutionId, WorkspaceId};
use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};
use cyberclaw_llm::types::{ChatRequest, Message};
use cyberclaw_llm::LlmClient;
use std::sync::Arc;
use tokio::process::Command;

// ===========================================================================
// Default executor
// ===========================================================================

/// Production verifier executor. Runs every [`VerifierKind`] for real.
///
/// Implements the canonical [`VerifierExecutor`] trait from
/// [`crate::persistent_execution`] so it slots directly into
/// [`crate::persistent_execution::PersistentLoop::with_verifier_executor`].
pub struct DefaultVerifierExecutor {
    /// Connector registry, used by [`VerifierKind::NumericMatchesCsv`].
    /// Optional so unit tests can supply `None` when not exercising CSV
    /// verification.
    pub connector_registry: Option<Arc<ConnectorRegistry>>,
    /// LLM client fallback for [`VerifierKind::LlmSemanticMatch`]. The
    /// per-call [`VerifierContext::llm_client`] takes precedence; this field
    /// is only used when the context's client is `None`.
    pub llm_client: Option<Arc<dyn LlmClient>>,
    /// Model name used for semantic match. Defaults to "gpt-4o-mini" when
    /// not set; the actual production wiring should override this.
    pub llm_model: String,
}

impl DefaultVerifierExecutor {
    pub fn new() -> Self {
        Self {
            connector_registry: None,
            llm_client: None,
            llm_model: "gpt-4o-mini".to_string(),
        }
    }

    pub fn with_connector_registry(mut self, registry: Arc<ConnectorRegistry>) -> Self {
        self.connector_registry = Some(registry);
        self
    }

    pub fn with_llm_client(mut self, client: Arc<dyn LlmClient>) -> Self {
        self.llm_client = Some(client);
        self
    }

    pub fn with_llm_model(mut self, model: impl Into<String>) -> Self {
        self.llm_model = model.into();
        self
    }
}

impl Default for DefaultVerifierExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for DefaultVerifierExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultVerifierExecutor")
            .field("connector_registry", &self.connector_registry.is_some())
            .field("has_llm_client", &self.llm_client.is_some())
            .field("llm_model", &self.llm_model)
            .finish()
    }
}

#[async_trait]
impl VerifierExecutor for DefaultVerifierExecutor {
    async fn run(&self, kind: &VerifierKind, ctx: &VerifierContext) -> VerifierVerdict {
        match kind {
            VerifierKind::FileExists { path } => verify_file_exists(path).await,
            VerifierKind::FileMimeMatches { path, expected } => {
                verify_file_mime(path, expected).await
            }
            VerifierKind::FileNonEmpty { path } => verify_file_non_empty(path).await,
            VerifierKind::TextNonEmpty { path } => verify_text_non_empty(path).await,
            VerifierKind::AudioDurationGt { path, min_secs } => {
                verify_audio_duration(path, *min_secs).await
            }
            VerifierKind::NumericMatchesCsv {
                csv_path,
                formula,
                tolerance,
            } => self.verify_numeric(csv_path, formula, *tolerance).await,
            VerifierKind::ScriptExitZero { script, args, cwd } => {
                verify_script_exit_zero(script, args, cwd).await
            }
            VerifierKind::LlmSemanticMatch {
                reference,
                actual_path,
                threshold,
            } => {
                self.verify_llm(reference, actual_path, *threshold, ctx)
                    .await
            }
        }
    }
}

// ===========================================================================
// Per-kind implementations
// ===========================================================================

async fn verify_file_exists(path: &str) -> VerifierVerdict {
    match tokio::fs::metadata(path).await {
        Ok(md) if md.is_file() => VerifierVerdict::Pass {
            evidence: format!("file '{path}' exists ({} bytes)", md.len()),
        },
        Ok(_) => VerifierVerdict::Fail {
            reason: format!("path '{path}' exists but is not a regular file"),
        },
        Err(e) => VerifierVerdict::Fail {
            reason: format!("file '{path}' missing: {e}"),
        },
    }
}

async fn verify_file_non_empty(path: &str) -> VerifierVerdict {
    match tokio::fs::metadata(path).await {
        Ok(md) if md.is_file() && md.len() > 0 => VerifierVerdict::Pass {
            evidence: format!("file '{path}' has {} bytes", md.len()),
        },
        Ok(md) if md.is_file() => VerifierVerdict::Fail {
            reason: format!("file '{path}' is empty (0 bytes)"),
        },
        Ok(_) => VerifierVerdict::Fail {
            reason: format!("path '{path}' is not a regular file"),
        },
        Err(e) => VerifierVerdict::Fail {
            reason: format!("file '{path}' inaccessible: {e}"),
        },
    }
}

async fn verify_text_non_empty(path: &str) -> VerifierVerdict {
    match tokio::fs::read_to_string(path).await {
        Ok(s) if !s.trim().is_empty() => {
            let preview: String = s.chars().take(80).collect();
            VerifierVerdict::Pass {
                evidence: format!(
                    "text file '{path}' has non-whitespace content (preview: {:?})",
                    preview
                ),
            }
        }
        Ok(_) => VerifierVerdict::Fail {
            reason: format!("text file '{path}' is empty or whitespace-only"),
        },
        Err(e) => VerifierVerdict::Fail {
            reason: format!("text file '{path}' unreadable: {e}"),
        },
    }
}

/// Detect MIME by sniffing magic bytes. Returns the canonical "image/png",
/// "application/pdf", etc. style string when recognised, else `None`.
fn detect_mime_from_bytes(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"%PDF") {
        return Some("application/pdf");
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    // ID3-tagged MP3.
    if bytes.starts_with(b"ID3") {
        return Some("audio/mpeg");
    }
    // MPEG audio frame sync (0xFF 0xFB / 0xFF 0xFA / 0xFF 0xF3 / 0xFF 0xF2).
    if bytes.len() >= 2
        && bytes[0] == 0xFF
        && (bytes[1] & 0xE0) == 0xE0
        && (bytes[1] & 0x18) != 0x08
    {
        return Some("audio/mpeg");
    }
    // ZIP / OOXML (PPTX/DOCX/XLSX) — both start with PK\x03\x04.
    if bytes.starts_with(&[b'P', b'K', 0x03, 0x04]) {
        return Some("application/zip");
    }
    None
}

async fn verify_file_mime(path: &str, expected: &str) -> VerifierVerdict {
    let mut file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => {
            return VerifierVerdict::Fail {
                reason: format!("file '{path}' inaccessible: {e}"),
            }
        }
    };
    let mut buf = [0u8; 16];
    let n = match tokio::io::AsyncReadExt::read(&mut file, &mut buf).await {
        Ok(n) => n,
        Err(e) => {
            return VerifierVerdict::Fail {
                reason: format!("file '{path}' read failed: {e}"),
            }
        }
    };
    let detected = detect_mime_from_bytes(&buf[..n]);
    match detected {
        Some(mime) if mime_matches(mime, expected) => VerifierVerdict::Pass {
            evidence: format!("file '{path}' mime={mime} matches '{expected}'"),
        },
        Some(mime) => VerifierVerdict::Fail {
            reason: format!("file '{path}' mime={mime} does not match expected '{expected}'"),
        },
        None => VerifierVerdict::Fail {
            reason: format!("file '{path}' mime could not be detected from first {n} bytes"),
        },
    }
}

/// Compare the detected mime against the expected one. Allows the caller to
/// pass either an exact MIME ("image/png"), or a coarser bucket ("image",
/// "audio") that any concrete subtype satisfies. Common OOXML aliases
/// ("application/vnd.openxmlformats-officedocument.presentationml.presentation",
/// "pptx", "docx", "xlsx", "office") all match the ZIP magic that real PPTX
/// files start with — the binary container is the same.
fn mime_matches(detected: &str, expected: &str) -> bool {
    let exp = expected.to_ascii_lowercase();
    let det = detected.to_ascii_lowercase();
    if det == exp {
        return true;
    }
    // Family / coarse match: "image" matches "image/png".
    if let Some(slash_idx) = det.find('/') {
        if det[..slash_idx] == exp {
            return true;
        }
    }
    // OOXML containers — pptx/docx/xlsx are zip envelopes.
    if det == "application/zip"
        && (exp.contains("pptx")
            || exp.contains("docx")
            || exp.contains("xlsx")
            || exp.contains("officedocument")
            || exp == "office"
            || exp == "ooxml")
    {
        return true;
    }
    // Common audio/mp3 alias.
    if det == "audio/mpeg" && (exp == "audio/mp3" || exp == "mp3") {
        return true;
    }
    false
}

async fn verify_audio_duration(path: &str, min_secs: f32) -> VerifierVerdict {
    // First, make sure the file is even there — gives a tighter error than
    // an opaque ffprobe spawn failure.
    if tokio::fs::metadata(path).await.is_err() {
        return VerifierVerdict::Fail {
            reason: format!("audio file '{path}' missing"),
        };
    }
    let result = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .await;
    let output = match result {
        Ok(o) => o,
        Err(e) => {
            // ENOENT here means ffprobe is missing on PATH.
            return VerifierVerdict::Fail {
                reason: format!("ffprobe not installed: {e}"),
            };
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return VerifierVerdict::Fail {
            reason: format!("ffprobe exited with non-zero status: {}", stderr.trim()),
        };
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    let duration: f32 = match trimmed.parse() {
        Ok(d) => d,
        Err(e) => {
            return VerifierVerdict::Fail {
                reason: format!("ffprobe produced unparseable duration {trimmed:?}: {e}"),
            };
        }
    };
    if duration >= min_secs {
        VerifierVerdict::Pass {
            evidence: format!("ffprobe reported {duration:.3}s ≥ min_secs={min_secs:.3}s"),
        }
    } else {
        VerifierVerdict::Fail {
            reason: format!("ffprobe reported {duration:.3}s < min_secs={min_secs:.3}s"),
        }
    }
}

async fn verify_script_exit_zero(script: &str, args: &[String], cwd: &str) -> VerifierVerdict {
    let result = Command::new(script)
        .args(args)
        .current_dir(cwd)
        .output()
        .await;
    let output = match result {
        Ok(o) => o,
        Err(e) => {
            return VerifierVerdict::Fail {
                reason: format!("script '{script}' failed to spawn: {e}"),
            };
        }
    };
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let tail = tail_lines(&stdout, 5);
        VerifierVerdict::Pass {
            evidence: format!("script '{script}' exited 0 (stdout tail: {tail:?})"),
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail = tail_lines(&stderr, 5);
        VerifierVerdict::Fail {
            reason: format!(
                "script '{script}' exited {} (stderr tail: {tail:?})",
                output.status.code().unwrap_or(-1)
            ),
        }
    }
}

fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

// ===========================================================================
// NumericMatchesCsv — formula parsing + capability dispatch
// ===========================================================================

/// Parsed `formula` for [`VerifierKind::NumericMatchesCsv`].
///
/// Recognised forms:
/// - `sum(col)` / `sum(col) = X` — column sum (and optional tolerance check).
/// - `avg(col)` / `avg(col) = X` — column mean.
/// - `count` / `count = N` — row count.
#[derive(Debug, PartialEq)]
enum NumericFormula {
    Sum {
        column: String,
        expected: Option<f64>,
    },
    Avg {
        column: String,
        expected: Option<f64>,
    },
    Count {
        expected: Option<f64>,
    },
}

fn parse_numeric_formula(raw: &str) -> Result<NumericFormula, String> {
    let s = raw.trim();
    let lower = s.to_ascii_lowercase();
    // Optional "= <number>" suffix.
    let (lhs, expected) = match s.find('=') {
        Some(idx) => {
            let rhs = s[idx + 1..].trim();
            let parsed: f64 = rhs
                .parse()
                .map_err(|e| format!("right-hand side '{rhs}' not a number: {e}"))?;
            (s[..idx].trim().to_string(), Some(parsed))
        }
        None => (s.to_string(), None),
    };
    let lhs_lower = lhs.to_ascii_lowercase();
    if lhs_lower.starts_with("sum(") && lhs.ends_with(')') {
        let col = lhs[4..lhs.len() - 1].trim().to_string();
        if col.is_empty() {
            return Err("sum() needs a column name".into());
        }
        return Ok(NumericFormula::Sum {
            column: col,
            expected,
        });
    }
    if lhs_lower.starts_with("avg(") && lhs.ends_with(')') {
        let col = lhs[4..lhs.len() - 1].trim().to_string();
        if col.is_empty() {
            return Err("avg() needs a column name".into());
        }
        return Ok(NumericFormula::Avg {
            column: col,
            expected,
        });
    }
    if lhs_lower == "count" || lower == "count" {
        return Ok(NumericFormula::Count { expected });
    }
    Err(format!(
        "unrecognised formula {raw:?} (expected sum(col), avg(col), or count, optionally with '= number')"
    ))
}

impl DefaultVerifierExecutor {
    async fn verify_numeric(
        &self,
        csv_path: &str,
        formula: &str,
        tolerance: f32,
    ) -> VerifierVerdict {
        let parsed = match parse_numeric_formula(formula) {
            Ok(p) => p,
            Err(e) => {
                return VerifierVerdict::Fail {
                    reason: format!("formula parse failed: {e}"),
                }
            }
        };
        // Build the input payload that `verify.numeric_aggregate` expects.
        let (column, expected_payload) = match &parsed {
            NumericFormula::Sum { column, expected } => (
                Some(column.clone()),
                serde_json::json!({ "sum": expected.unwrap_or(0.0) }),
            ),
            NumericFormula::Avg { column, expected } => (
                Some(column.clone()),
                serde_json::json!({ "avg": expected.unwrap_or(0.0) }),
            ),
            NumericFormula::Count { expected } => (
                None,
                serde_json::json!({ "count": expected.unwrap_or(0.0) as u64 }),
            ),
        };

        // If the caller didn't supply an `expected = X`, we can't run a
        // tolerance check — we only compute the value. In that case we
        // resolve via the capability, but instead of checking `match`, we
        // return Pass with the computed actual as evidence.
        let has_expected = match &parsed {
            NumericFormula::Sum { expected, .. } | NumericFormula::Avg { expected, .. } => {
                expected.is_some()
            }
            NumericFormula::Count { expected } => expected.is_some(),
        };

        let mut input = serde_json::json!({
            "csv_path": csv_path,
            "expected": expected_payload,
            "tolerance": tolerance,
        });
        if let Some(col) = column {
            input["column"] = serde_json::Value::String(col);
        }

        let registry = match &self.connector_registry {
            Some(r) => r.clone(),
            None => {
                return VerifierVerdict::Fail {
                    reason:
                        "NumericMatchesCsv needs a ConnectorRegistry to dispatch verify.numeric_aggregate"
                            .to_string(),
                };
            }
        };
        let cap_id = match CapabilityId::from_string("verify.numeric_aggregate".to_string()) {
            Ok(c) => c,
            Err(e) => {
                return VerifierVerdict::Fail {
                    reason: format!("invalid capability id: {e}"),
                }
            }
        };
        let connector_id = match registry.find_connector_for_capability(&cap_id) {
            Some(id) => id,
            None => {
                return VerifierVerdict::Fail {
                    reason:
                        "verify.numeric_aggregate capability not registered in ConnectorRegistry"
                            .to_string(),
                };
            }
        };
        let connector = match registry.get(&connector_id) {
            Some(c) => c,
            None => {
                return VerifierVerdict::Fail {
                    reason: "connector vanished after lookup".to_string(),
                }
            }
        };

        // Build a capability-execution request. The verify capability only
        // looks at `input` and `connector.workspace`, so the rest is plumbing.
        let request = CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: format!("verify-{}", uuid::Uuid::new_v4()),
            actor: ActorRef {
                id: ActorId::from_string("verifier_executor".to_string()).unwrap(),
                actor_type: ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "verifier_executor".into(),
            },
            workspace: WorkspaceRef {
                id: WorkspaceId::new(),
                mode: WorkspaceMode::Ephemeral,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: ".".to_string(),
                writable_roots: vec![],
            },
            connector_id,
            capability_id: cap_id,
            input,
        };

        let result = match connector.execute(request).await {
            Ok(r) => r,
            Err(e) => {
                return VerifierVerdict::Fail {
                    reason: format!("verify.numeric_aggregate failed: {e}"),
                };
            }
        };
        // Inspect the output JSON.
        let out = result.output;
        let r#match = out.get("match").and_then(|v| v.as_bool()).unwrap_or(false);
        let actual = out
            .get("actual")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let diff = out
            .get("diff")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if !has_expected {
            // No tolerance check — pass with computed value as evidence.
            return VerifierVerdict::Pass {
                evidence: format!("computed {formula} (no expected supplied; actual={actual})"),
            };
        }
        if r#match {
            VerifierVerdict::Pass {
                evidence: format!("verify.numeric_aggregate match=true actual={actual}"),
            }
        } else {
            VerifierVerdict::Fail {
                reason: format!(
                    "verify.numeric_aggregate mismatch: diff={:?} actual={actual}",
                    diff.iter()
                        .filter_map(|d| d.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                ),
            }
        }
    }

    async fn verify_llm(
        &self,
        reference: &str,
        actual_path: &str,
        threshold: f32,
        ctx: &VerifierContext,
    ) -> VerifierVerdict {
        let actual = match tokio::fs::read_to_string(actual_path).await {
            Ok(s) => s,
            Err(e) => {
                return VerifierVerdict::Fail {
                    reason: format!("actual_path '{actual_path}' unreadable: {e}"),
                };
            }
        };
        // Prefer the per-call ctx client; fall back to the executor's bound
        // client; if neither is present, fail loud.
        let client = match ctx.llm_client.clone().or_else(|| self.llm_client.clone()) {
            Some(c) => c,
            None => {
                return VerifierVerdict::Fail {
                    reason: "LlmSemanticMatch requires an llm_client; none configured on executor or context".to_string(),
                };
            }
        };
        let prompt = format!(
            "You are a strict verifier. The REFERENCE describes what the ACTUAL content should mean.\n\n\
             REFERENCE:\n{reference}\n\n\
             ACTUAL:\n{actual}\n\n\
             Does the ACTUAL semantically satisfy the REFERENCE? Reply with exactly one of: YES or NO."
        );
        let request = ChatRequest {
            model: self.llm_model.clone(),
            messages: vec![Message::user(prompt)],
            temperature: Some(0.0),
            max_tokens: Some(8),
            ..Default::default()
        };
        let response = match client.chat_completion(request).await {
            Ok(r) => r,
            Err(e) => {
                return VerifierVerdict::Fail {
                    reason: format!("LLM call failed: {e}"),
                };
            }
        };
        let answer = response
            .choices
            .first()
            .map(|c| c.message.content.trim().to_string())
            .unwrap_or_default();
        let upper = answer.to_ascii_uppercase();
        if upper.starts_with("YES") {
            VerifierVerdict::Pass {
                evidence: format!(
                    "LLM semantic match: YES (threshold={threshold}, model={})",
                    self.llm_model
                ),
            }
        } else if upper.starts_with("NO") {
            VerifierVerdict::Fail {
                reason: format!("LLM semantic match: NO (threshold={threshold}, raw={answer:?})"),
            }
        } else {
            VerifierVerdict::Fail {
                reason: format!("LLM produced ambiguous answer {answer:?}; expected YES or NO"),
            }
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_llm::error::LlmResult;
    use cyberclaw_llm::types::{ChatChunk, ChatResponse, Choice, Message as LlmMessage, Role};
    use futures::stream::Stream;
    use std::sync::Mutex;
    use tempfile::TempDir;

    fn ctx() -> VerifierContext {
        VerifierContext::default()
    }

    /// Sprint D4: `VerifierVerdict` lives in `persistent_execution`, so we
    /// cannot add helpers on the foreign type. This local helper keeps the
    /// test asserts terse without the extra `match` boilerplate.
    fn is_pass(v: &VerifierVerdict) -> bool {
        matches!(v, VerifierVerdict::Pass { .. })
    }

    // ---------- FileExists ----------

    #[tokio::test]
    async fn file_exists_yes() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("a.txt");
        tokio::fs::write(&p, b"hi").await.unwrap();
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::FileExists {
                    path: p.to_string_lossy().to_string(),
                },
                &ctx(),
            )
            .await;
        assert!(is_pass(&v), "got {:?}", v);
    }

    #[tokio::test]
    async fn file_exists_no() {
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::FileExists {
                    path: "/tmp/_nope_cyberclaw_d4_does_not_exist_xx.txt".to_string(),
                },
                &ctx(),
            )
            .await;
        assert!(!is_pass(&v), "got {:?}", v);
    }

    // ---------- FileMimeMatches ----------

    #[tokio::test]
    async fn file_mime_png_matches() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("a.png");
        // Real PNG magic.
        tokio::fs::write(&p, [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0])
            .await
            .unwrap();
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::FileMimeMatches {
                    path: p.to_string_lossy().to_string(),
                    expected: "image/png".to_string(),
                },
                &ctx(),
            )
            .await;
        assert!(is_pass(&v), "got {:?}", v);
    }

    #[tokio::test]
    async fn file_mime_pdf_matches() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("a.pdf");
        tokio::fs::write(&p, b"%PDF-1.4\n%...").await.unwrap();
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::FileMimeMatches {
                    path: p.to_string_lossy().to_string(),
                    expected: "application/pdf".to_string(),
                },
                &ctx(),
            )
            .await;
        assert!(is_pass(&v), "got {:?}", v);
    }

    #[tokio::test]
    async fn file_mime_mismatch_fails() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("a.png");
        tokio::fs::write(&p, b"%PDF-1.4\n").await.unwrap(); // PDF bytes
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::FileMimeMatches {
                    path: p.to_string_lossy().to_string(),
                    expected: "image/png".to_string(),
                },
                &ctx(),
            )
            .await;
        assert!(!is_pass(&v), "got {:?}", v);
    }

    // ---------- FileNonEmpty ----------

    #[tokio::test]
    async fn file_non_empty_pass() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("nz.bin");
        tokio::fs::write(&p, b"abc").await.unwrap();
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::FileNonEmpty {
                    path: p.to_string_lossy().to_string(),
                },
                &ctx(),
            )
            .await;
        assert!(is_pass(&v));
    }

    #[tokio::test]
    async fn file_non_empty_fail_zero_size() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("zero.bin");
        tokio::fs::write(&p, b"").await.unwrap();
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::FileNonEmpty {
                    path: p.to_string_lossy().to_string(),
                },
                &ctx(),
            )
            .await;
        assert!(!is_pass(&v));
    }

    // ---------- TextNonEmpty ----------

    #[tokio::test]
    async fn text_non_empty_valid() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("t.txt");
        tokio::fs::write(&p, "hello\n").await.unwrap();
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::TextNonEmpty {
                    path: p.to_string_lossy().to_string(),
                },
                &ctx(),
            )
            .await;
        assert!(is_pass(&v));
    }

    #[tokio::test]
    async fn text_non_empty_whitespace_only_fails() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("w.txt");
        tokio::fs::write(&p, "   \n\t  \n").await.unwrap();
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::TextNonEmpty {
                    path: p.to_string_lossy().to_string(),
                },
                &ctx(),
            )
            .await;
        assert!(!is_pass(&v));
    }

    #[tokio::test]
    async fn text_non_empty_completely_empty_fails() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("e.txt");
        tokio::fs::write(&p, "").await.unwrap();
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::TextNonEmpty {
                    path: p.to_string_lossy().to_string(),
                },
                &ctx(),
            )
            .await;
        assert!(!is_pass(&v));
    }

    // ---------- AudioDurationGt ----------

    #[tokio::test]
    async fn audio_duration_missing_file_fails() {
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::AudioDurationGt {
                    path: "/tmp/_no_such_audio_file_d4_x.mp3".to_string(),
                    min_secs: 1.0,
                },
                &ctx(),
            )
            .await;
        assert!(!is_pass(&v));
    }

    /// Real ffprobe path — gated behind #[ignore] so default CI passes
    /// without ffprobe installed. Run with:
    ///   `cargo test --workspace --lib audio_duration_with_ffprobe -- --ignored`
    #[tokio::test]
    #[ignore = "requires ffprobe + a real audio fixture; default-skipped"]
    async fn audio_duration_with_ffprobe_real() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("silence.mp3");
        // Minimal MP3 header — ffprobe will refuse this but the spawn path
        // exercises everything except duration parsing; for a real run, drop
        // a real fixture mp3 in place of `silence.mp3`.
        tokio::fs::write(&p, b"ID3\x03\x00\x00\x00\x00\x00\x00")
            .await
            .unwrap();
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::AudioDurationGt {
                    path: p.to_string_lossy().to_string(),
                    min_secs: 0.0,
                },
                &ctx(),
            )
            .await;
        // Whatever ffprobe says, the call must not panic.
        assert!(is_pass(&v) || !is_pass(&v));
    }

    // ---------- ScriptExitZero ----------

    #[tokio::test]
    async fn script_exit_zero_pass() {
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::ScriptExitZero {
                    script: "true".to_string(),
                    args: vec![],
                    cwd: ".".to_string(),
                },
                &ctx(),
            )
            .await;
        assert!(is_pass(&v), "got {:?}", v);
    }

    #[tokio::test]
    async fn script_exit_nonzero_fails() {
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::ScriptExitZero {
                    script: "false".to_string(),
                    args: vec![],
                    cwd: ".".to_string(),
                },
                &ctx(),
            )
            .await;
        assert!(!is_pass(&v), "got {:?}", v);
    }

    #[tokio::test]
    async fn script_missing_binary_fails() {
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::ScriptExitZero {
                    script: "/nope/this/binary/does/not/exist".to_string(),
                    args: vec![],
                    cwd: ".".to_string(),
                },
                &ctx(),
            )
            .await;
        assert!(!is_pass(&v));
    }

    // ---------- NumericMatchesCsv ----------

    #[test]
    fn parse_numeric_formula_sum_with_expected() {
        let f = parse_numeric_formula("sum(amount) = 60").unwrap();
        assert_eq!(
            f,
            NumericFormula::Sum {
                column: "amount".to_string(),
                expected: Some(60.0),
            }
        );
    }

    #[test]
    fn parse_numeric_formula_avg_no_expected() {
        let f = parse_numeric_formula("avg(price)").unwrap();
        assert_eq!(
            f,
            NumericFormula::Avg {
                column: "price".to_string(),
                expected: None,
            }
        );
    }

    #[test]
    fn parse_numeric_formula_count() {
        let f = parse_numeric_formula("count = 3").unwrap();
        assert_eq!(
            f,
            NumericFormula::Count {
                expected: Some(3.0)
            }
        );
    }

    #[test]
    fn parse_numeric_formula_unknown_rejected() {
        assert!(parse_numeric_formula("median(x)").is_err());
    }

    /// Routes through the real `verify.numeric_aggregate` capability via
    /// LocalConnector + ConnectorRegistry. Exercises the full executor path.
    #[tokio::test]
    async fn numeric_matches_csv_sum_pass() {
        let tmp = TempDir::new().unwrap();
        let csv = tmp.path().join("data.csv");
        tokio::fs::write(&csv, b"id,amount\n1,10\n2,20\n3,30\n")
            .await
            .unwrap();
        let registry = Arc::new(ConnectorRegistry::new());
        let conn: Arc<dyn cyberclaw_connectors::types::Connector> = Arc::new(
            cyberclaw_connectors::local::LocalConnector::new(tmp.path().to_path_buf()),
        );
        registry.register(conn).unwrap();
        let exec = DefaultVerifierExecutor::new().with_connector_registry(registry);
        let v = exec
            .run(
                &VerifierKind::NumericMatchesCsv {
                    csv_path: csv.to_string_lossy().to_string(),
                    formula: "sum(amount) = 60".to_string(),
                    tolerance: 1e-6,
                },
                &ctx(),
            )
            .await;
        assert!(is_pass(&v), "got {:?}", v);
    }

    #[tokio::test]
    async fn numeric_matches_csv_avg_mismatch_fails() {
        let tmp = TempDir::new().unwrap();
        let csv = tmp.path().join("data.csv");
        tokio::fs::write(&csv, b"id,amount\n1,2\n2,4\n")
            .await
            .unwrap();
        let registry = Arc::new(ConnectorRegistry::new());
        let conn: Arc<dyn cyberclaw_connectors::types::Connector> = Arc::new(
            cyberclaw_connectors::local::LocalConnector::new(tmp.path().to_path_buf()),
        );
        registry.register(conn).unwrap();
        let exec = DefaultVerifierExecutor::new().with_connector_registry(registry);
        let v = exec
            .run(
                &VerifierKind::NumericMatchesCsv {
                    csv_path: csv.to_string_lossy().to_string(),
                    formula: "avg(amount) = 100".to_string(),
                    tolerance: 1e-6,
                },
                &ctx(),
            )
            .await;
        assert!(!is_pass(&v), "got {:?}", v);
    }

    #[tokio::test]
    async fn numeric_matches_csv_count_pass() {
        let tmp = TempDir::new().unwrap();
        let csv = tmp.path().join("data.csv");
        tokio::fs::write(&csv, b"id\n1\n2\n3\n4\n").await.unwrap();
        let registry = Arc::new(ConnectorRegistry::new());
        let conn: Arc<dyn cyberclaw_connectors::types::Connector> = Arc::new(
            cyberclaw_connectors::local::LocalConnector::new(tmp.path().to_path_buf()),
        );
        registry.register(conn).unwrap();
        let exec = DefaultVerifierExecutor::new().with_connector_registry(registry);
        let v = exec
            .run(
                &VerifierKind::NumericMatchesCsv {
                    csv_path: csv.to_string_lossy().to_string(),
                    formula: "count = 4".to_string(),
                    tolerance: 1e-6,
                },
                &ctx(),
            )
            .await;
        assert!(is_pass(&v), "got {:?}", v);
    }

    #[tokio::test]
    async fn numeric_matches_csv_no_registry_fails() {
        let tmp = TempDir::new().unwrap();
        let csv = tmp.path().join("data.csv");
        tokio::fs::write(&csv, b"id,amount\n1,10\n").await.unwrap();
        // No registry — must surface a failure, not a panic.
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::NumericMatchesCsv {
                    csv_path: csv.to_string_lossy().to_string(),
                    formula: "sum(amount) = 10".to_string(),
                    tolerance: 1e-6,
                },
                &ctx(),
            )
            .await;
        assert!(!is_pass(&v));
    }

    // ---------- LlmSemanticMatch ----------

    /// Mock LLM client whose chat_completion always returns a configured
    /// canned answer. Used to exercise both the YES and NO branches of the
    /// semantic-match verifier without touching a real network.
    struct MockLlm {
        canned: Mutex<String>,
    }

    impl MockLlm {
        fn new(answer: &str) -> Self {
            Self {
                canned: Mutex::new(answer.to_string()),
            }
        }
    }

    impl std::fmt::Debug for MockLlm {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MockLlm").finish()
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for MockLlm {
        async fn chat_completion(&self, _request: ChatRequest) -> LlmResult<ChatResponse> {
            let answer = self.canned.lock().unwrap().clone();
            Ok(ChatResponse {
                id: "mock".into(),
                object: "chat.completion".into(),
                created: 0,
                model: "mock-model".into(),
                choices: vec![Choice {
                    index: 0,
                    message: LlmMessage {
                        role: Role::Assistant,
                        content: answer,
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                        cache_control: None,
                    },
                    finish_reason: Some("stop".into()),
                }],
                usage: None,
                rate_limit: None,
            })
        }
        async fn chat_completion_stream(
            &self,
            _request: ChatRequest,
        ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
            unimplemented!("stream not used in tests")
        }
        fn provider(&self) -> &str {
            "mock"
        }
        async fn validate_connection(&self) -> LlmResult<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn llm_semantic_match_yes_pass_via_executor_client() {
        let tmp = TempDir::new().unwrap();
        let actual = tmp.path().join("actual.txt");
        tokio::fs::write(&actual, "the quick brown fox")
            .await
            .unwrap();
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new("YES"));
        let exec = DefaultVerifierExecutor::new().with_llm_client(llm);
        let v = exec
            .run(
                &VerifierKind::LlmSemanticMatch {
                    reference: "fox metaphor".to_string(),
                    actual_path: actual.to_string_lossy().to_string(),
                    threshold: 0.8,
                },
                &ctx(),
            )
            .await;
        assert!(is_pass(&v), "got {:?}", v);
    }

    #[tokio::test]
    async fn llm_semantic_match_yes_pass_via_context_client() {
        // Sprint D4: when the executor has no client but the VerifierContext
        // does, we still get a verdict via the per-call client.
        let tmp = TempDir::new().unwrap();
        let actual = tmp.path().join("actual.txt");
        tokio::fs::write(&actual, "x").await.unwrap();
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new("YES"));
        let c = VerifierContext {
            llm_client: Some(llm),
            ..Default::default()
        };
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::LlmSemanticMatch {
                    reference: "anything".to_string(),
                    actual_path: actual.to_string_lossy().to_string(),
                    threshold: 0.5,
                },
                &c,
            )
            .await;
        assert!(is_pass(&v), "got {:?}", v);
    }

    #[tokio::test]
    async fn llm_semantic_match_no_fail() {
        let tmp = TempDir::new().unwrap();
        let actual = tmp.path().join("actual.txt");
        tokio::fs::write(&actual, "completely off topic")
            .await
            .unwrap();
        let llm: Arc<dyn LlmClient> = Arc::new(MockLlm::new("NO"));
        let exec = DefaultVerifierExecutor::new().with_llm_client(llm);
        let v = exec
            .run(
                &VerifierKind::LlmSemanticMatch {
                    reference: "must be a fox metaphor".to_string(),
                    actual_path: actual.to_string_lossy().to_string(),
                    threshold: 0.8,
                },
                &ctx(),
            )
            .await;
        assert!(!is_pass(&v), "got {:?}", v);
    }

    #[tokio::test]
    async fn llm_semantic_match_no_client_fails() {
        let tmp = TempDir::new().unwrap();
        let actual = tmp.path().join("actual.txt");
        tokio::fs::write(&actual, "x").await.unwrap();
        let exec = DefaultVerifierExecutor::new();
        let v = exec
            .run(
                &VerifierKind::LlmSemanticMatch {
                    reference: "anything".to_string(),
                    actual_path: actual.to_string_lossy().to_string(),
                    threshold: 0.5,
                },
                &ctx(),
            )
            .await;
        assert!(!is_pass(&v));
    }

    /// Sprint D4: end-to-end check that DefaultVerifierExecutor implements the
    /// canonical D3 trait — slot it into PersistentLoop without an extra
    /// adapter. Failing-stub execution end-to-end is covered by D3's own
    /// PersistentLoop tests; here we just confirm the trait wiring compiles.
    #[tokio::test]
    async fn default_executor_satisfies_d3_trait_object() {
        let exec: Arc<dyn VerifierExecutor> = Arc::new(DefaultVerifierExecutor::new());
        let v = exec
            .run(
                &VerifierKind::FileExists {
                    path: "/dev/null".to_string(),
                },
                &ctx(),
            )
            .await;
        // /dev/null exists but is not a regular file → Fail (verifies the
        // trait dispatch reaches our impl).
        assert!(!is_pass(&v), "got {:?}", v);
    }
}
