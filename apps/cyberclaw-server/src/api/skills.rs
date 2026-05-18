//! Skills API — remote + local skill hub surface.
//!
//! Sprint 7. Backs the admin console's Skills page. Wraps
//! [`cyberclaw_skill_runtime::skill_hub::SkillHub`] so every GET / POST /
//! DELETE maps to a real hub operation (discover, install, remove) with a
//! full audit trail on disk.
//!
//! # Routes
//!
//! | Route | Purpose |
//! |---|---|
//! | `GET    /api/v1/skills` | List all installed skills (from hub lock file) |
//! | `POST   /api/v1/skills/install` | Download + scan + install a bundle |
//! | `GET    /api/v1/skills/:id/content` | Return the skill's `SKILL.md` body |
//! | `DELETE /api/v1/skills/:id` | Uninstall a skill |
//!
//! # Shape contract
//!
//! Each record returned is `SkillView { skill_id, name, category, source,
//! description, installed_at }` — shape mirrors the frontend
//! `MOCK.skills` entries in `web/src/data.jsx`.

use std::sync::Arc;

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use cyberclaw_control_plane::intent_classifier::{Intent, IntentClassifier};
use cyberclaw_skill_runtime::skill_hub::{SkillBundle, SkillSource, SkillState};
use cyberclaw_skill_runtime::skill_scanner::{SkillScanner, SkillTrustLevel};

use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::{require_admin, Claims};
use crate::state::AppState;

/// Admin-visible projection of a skill.
#[derive(Debug, Clone, Serialize)]
pub struct SkillView {
    pub skill_id: String,
    pub name: String,
    pub category: String,
    /// `"builtin" | "hub" | "local"` — classified from the source string.
    pub source: String,
    pub description: String,
    pub installed_at: String,
    /// Per-skill admin toggle. For seeded skills this is the in-memory flag;
    /// for filesystem-installed skills it is computed from a `.disabled` marker
    /// under `<hub_base>/disabled/<name>`.
    pub enabled: bool,
    // --- CyberClaw-specific enrichment fields ---
    /// Semantic version from SKILL.md frontmatter; None if absent.
    pub version: Option<String>,
    /// Tags from SKILL.md frontmatter `tags:` field; empty if absent.
    pub tags: Vec<String>,
    /// Author from SKILL.md frontmatter `author:` field; None if absent.
    pub author: Option<String>,
    /// First 200–500 chars of SKILL.md body (frontmatter stripped, plain text, no HTML).
    pub readme_excerpt: String,
    /// Explicit trust decision based on source:
    /// "high" (builtin) | "medium" (hub) | "low" (local).
    pub trust_level: String,
    /// Total bytes of all files in the skill directory.
    pub size_bytes: u64,
    /// Number of files in the skill directory.
    pub file_count: u32,
    /// Whether the skill directory contains a `scripts/` subdirectory
    /// (indicates executable risk — surface to admin).
    pub has_scripts: bool,
    /// Optional Chinese-language description (from SKILL.md frontmatter
    /// `description_zh:`). UI uses this when lang=zh-CN; otherwise falls
    /// back to `description`.
    pub description_zh: Option<String>,
    /// Origin / provenance — where the skill was ported from. Read from
    /// `metadata.cyberclaw.source` in SKILL.md frontmatter. e.g.
    /// "deer-flow/skills/public/academic-paper-review".
    pub origin: Option<String>,
    /// When the skill was first imported into this CyberClaw instance.
    /// Read from `metadata.cyberclaw.imported_at` in SKILL.md frontmatter.
    /// Distinct from `installed_at` which tracks installation into this
    /// specific server's SkillHub.
    pub imported_at_origin: Option<String>,
}

/// `POST /api/v1/skills/install` body.
#[derive(Debug, Deserialize)]
pub struct InstallSkillRequest {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_source_label")]
    pub source: String,
    #[serde(default)]
    pub local_path: Option<String>,
}

/// `POST /api/v1/skills/create` body.
///
/// Entry point for "the user wants a new Skill". Produces a SKILL.md scaffold
/// under `~/.cyberclaw/skills/{name}/` and installs it via `SkillHub`. The
/// `description` is classified through [`IntentClassifier`] so non-skill
/// requests can be rejected at the API boundary (provenance field
/// `classified_intent` surfaces the verdict).
///
/// Real EvolutionOrchestrator-driven optimisation (description tuning,
/// trigger eval loop) is deferred to a background job triggered separately
/// — this endpoint only lays down the scaffold.
#[derive(Debug, Deserialize)]
pub struct CreateSkillRequest {
    /// Slug (`^[a-z][a-z0-9-]{2,31}$`). Mirrors the frontend validation.
    pub name: String,
    /// Natural-language description of what the skill should do.
    pub description: String,
    /// Optional prose body — becomes the "# Methodology" section.
    #[serde(default)]
    pub methodology: Option<String>,
    /// Example prompts that should trigger this skill (for later eval).
    #[serde(default)]
    pub trigger_examples: Vec<String>,
    /// Sprint 21 — provenance trail. When the skill was extracted from
    /// an agentic conversation, the caller can attach the originating
    /// `request:<uuid>` so the audit chain links the skill back to its
    /// source experience. The id is persisted into the SKILL.md
    /// frontmatter as `source_request_id:` for offline forensics, and
    /// echoed into the audit row for the create event.
    #[serde(default)]
    pub source_request_id: Option<String>,
}

/// `POST /api/v1/skills/install-remote` body.
///
/// Accepts either a GitHub repository in `owner/repo` form (optionally with
/// `#branch`) or a fully-qualified tarball URL. Expected `sha256` is passed
/// through to `SkillHub::download` for integrity verification.
#[derive(Debug, Deserialize)]
pub struct InstallRemoteSkillRequest {
    /// Skill name used as the hub entry and quarantine directory.
    pub name: String,
    /// `"github"` or `"registry"`. Defaults to `"github"`.
    #[serde(default = "default_remote_kind")]
    pub kind: String,
    /// For `kind="github"`: `owner/repo` (optionally `owner/repo#branch`).
    /// For `kind="registry"`: full tarball URL (.tar.gz).
    pub url: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Optional sha256 for integrity verification.
    #[serde(default)]
    pub sha256: Option<String>,
}

fn default_remote_kind() -> String {
    "github".to_string()
}

fn default_version() -> String {
    "0.0.0".to_string()
}

fn default_source_label() -> String {
    "local".to_string()
}

/// `GET /api/v1/skills/:id/content` response.
#[derive(Debug, Serialize)]
pub struct SkillContentResponse {
    pub skill_id: String,
    pub content: String,
}

/// Build the `/api/v1/skills` router.
pub fn create_skills_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/skills", get(list_skills))
        .route("/api/v1/skills/install", post(install_skill))
        .route("/api/v1/skills/install-remote", post(install_skill_remote))
        .route("/api/v1/skills/create", post(create_skill))
        // Sprint 21 path #3 — must be registered BEFORE the `/:id/content`
        // dynamic match so axum routes "search" as a static path.
        .route("/api/v1/skills/search", get(search_skills))
        // Public registry listing — unauthenticated-friendly (still under JWT lane
        // for admin console; the shape mirrors the installable bundle catalog).
        .route("/api/v1/skills/registry", get(list_registry))
        // Publisher key ring management (admin only for POST).
        .route(
            "/api/v1/skills/publisher-keys",
            get(list_publisher_keys).post(add_publisher_key),
        )
        .route("/api/v1/skills/:id/content", get(get_skill_content))
        .route("/api/v1/skills/:id/toggle", post(toggle_skill))
        .route("/api/v1/skills/:id", delete(uninstall_skill))
}

/// Classify a raw source string into one of `builtin | hub | local`.
///
/// Recognised prefixes/values:
/// - `"builtin"` or `"builtin:*"` → `"builtin"`
/// - `"github:*"`, `"registry:*"`, `"ecosystem:*"`, `"hub"` → `"hub"`
/// - anything else → `"local"`
fn classify_source(source: &str) -> String {
    if source == "builtin" || source.starts_with("builtin:") {
        "builtin".to_string()
    } else if source.starts_with("github:")
        || source.starts_with("registry:")
        || source.starts_with("ecosystem:")
        || source == "hub"
    {
        "hub".to_string()
    } else {
        "local".to_string()
    }
}

/// Adapt an internal [`SkillBundle`] into an admin-facing [`SkillView`].
///
/// Enrichment pass: reads SKILL.md from the installed directory to extract
/// version, tags, author, and readme_excerpt. Falls back to empty/None on
/// any I/O or parse failure — the API never errors on missing metadata.
#[cfg_attr(not(test), allow(dead_code))]
fn to_admin_view(bundle: &SkillBundle, installed_at: Option<String>, enabled: bool) -> SkillView {
    to_admin_view_with_base(bundle, installed_at, enabled, None)
}

/// Variant that accepts an explicit hub base_dir so callers with a live hub
/// reference can trigger the enrichment path without re-constructing the path.
fn to_admin_view_with_base(
    bundle: &SkillBundle,
    installed_at: Option<String>,
    enabled: bool,
    hub_base: Option<&std::path::Path>,
) -> SkillView {
    let classified_source = classify_source(&bundle.source);
    let trust_level = trust_level_from_source(&classified_source);

    // Attempt SKILL.md enrichment from the installed directory.
    let (meta, size_bytes, file_count, has_scripts) = if let Some(base) = hub_base {
        let skill_dir = base.join("installed").join(&bundle.name);
        let skill_md = skill_dir.join("SKILL.md");
        let meta = if skill_md.exists() {
            parse_skill_md(&skill_md)
        } else {
            SkillMeta {
                version: None,
                tags: vec![],
                author: None,
                description: None,
                description_zh: None,
                origin: None,
                imported_at_origin: None,
                readme_excerpt: String::new(),
            }
        };
        let (sz, fc) = if skill_dir.exists() {
            skill_dir_stats(&skill_dir)
        } else {
            (0, 0)
        };
        let has_scripts = skill_dir.join("scripts").is_dir();
        (meta, sz, fc, has_scripts)
    } else {
        (
            SkillMeta {
                version: None,
                tags: vec![],
                author: None,
                description: None,
                description_zh: None,
                origin: None,
                imported_at_origin: None,
                readme_excerpt: String::new(),
            },
            0,
            0,
            false,
        )
    };

    // Prefer bundle.description (from lock file / install request) if non-empty;
    // otherwise fall back to what SKILL.md frontmatter provides.
    let description = if !bundle.description.is_empty() {
        bundle.description.clone()
    } else {
        meta.description.unwrap_or_default()
    };

    SkillView {
        skill_id: format!("sk_{}", bundle.name),
        name: bundle.name.clone(),
        category: infer_category(&bundle.name),
        source: classified_source,
        description,
        installed_at: installed_at.unwrap_or_default(),
        enabled,
        version: meta.version,
        tags: meta.tags,
        author: meta.author,
        readme_excerpt: meta.readme_excerpt,
        trust_level,
        size_bytes,
        file_count,
        has_scripts,
        description_zh: meta.description_zh,
        origin: meta.origin,
        imported_at_origin: meta.imported_at_origin,
    }
}

// ---------------------------------------------------------------------------
// SKILL.md enrichment helpers
// ---------------------------------------------------------------------------

/// Parsed metadata from a SKILL.md file's YAML frontmatter.
pub struct SkillMeta {
    pub version: Option<String>,
    pub tags: Vec<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    /// Optional Chinese-language description; surfaced when the UI runs in zh-CN.
    /// Falls back to `description` if absent.
    pub description_zh: Option<String>,
    /// Origin/provenance — where the skill was imported from (e.g. upstream
    /// repo path or URL). Read from `metadata.cyberclaw.source` in frontmatter.
    pub origin: Option<String>,
    /// When the skill was imported into this CyberClaw instance. Read from
    /// `metadata.cyberclaw.imported_at` in frontmatter.
    pub imported_at_origin: Option<String>,
    /// First 200-500 chars of the markdown body (frontmatter stripped).
    pub readme_excerpt: String,
}

/// Parse a SKILL.md file's frontmatter and extract the first content paragraph
/// as the readme_excerpt. Returns defaults on any parse failure.
///
/// Safety: output strings are plain text only — no HTML tags. The excerpt is
/// truncated at 500 characters to bound output size.
pub fn parse_skill_md(path: &std::path::Path) -> SkillMeta {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => {
            return SkillMeta {
                version: None,
                tags: vec![],
                author: None,
                description: None,
                description_zh: None,
                origin: None,
                imported_at_origin: None,
                readme_excerpt: String::new(),
            }
        }
    };

    // Detect YAML frontmatter: starts with "---\n" and has a closing "---".
    let (frontmatter_str, body) = if content.starts_with("---\n") || content.starts_with("---\r\n")
    {
        // Find the closing delimiter after the opening "---"
        let rest = &content[3..]; // skip "---"
        let rest = rest.trim_start_matches('\n').trim_start_matches("\r\n");
        if let Some(end_idx) = rest.find("\n---") {
            let fm = &rest[..end_idx];
            let after = &rest[end_idx + 4..]; // skip "\n---"
                                              // Some SKILL.md files have a double "---\n---\n" pattern; skip blank lines
            let after = after.trim_start_matches('\n').trim_start_matches("\r\n");
            // If the body itself starts with "---", skip that too (double frontmatter delimiter)
            let body = if after.starts_with("---") {
                after
                    .trim_start_matches("---")
                    .trim_start_matches('\n')
                    .trim_start_matches("\r\n")
            } else {
                after
            };
            (Some(fm), body.to_string())
        } else {
            (None, content.clone())
        }
    } else {
        (None, content.clone())
    };

    let mut version = None;
    let mut tags: Vec<String> = vec![];
    let mut author = None;
    let mut description = None;
    let mut description_zh = None;
    let mut origin = None;
    let mut imported_at_origin = None;

    if let Some(fm) = frontmatter_str {
        // Use serde_yaml to parse the frontmatter.
        if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(fm) {
            if let Some(map) = yaml.as_mapping() {
                version = map
                    .get("version")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                author = map
                    .get("author")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                description = map
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                // Optional Chinese description — accepts either `description_zh:`
                // (preferred) or `description_zh_cn:` (legacy alias).
                description_zh = map
                    .get("description_zh")
                    .or_else(|| map.get("description_zh_cn"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                // Provenance: read nested `metadata.cyberclaw.{source,imported_at}`
                // which is the convention used by skills ported from upstream
                // repos (deer-flow, hermes-agent, etc.). The flat top-level
                // `metadata.source` and `metadata.imported_at` are also accepted
                // as a more forgiving fallback.
                if let Some(metadata) = map.get("metadata").and_then(|v| v.as_mapping()) {
                    let cc = metadata.get("cyberclaw").and_then(|v| v.as_mapping());
                    origin = cc
                        .and_then(|m| m.get("source"))
                        .or_else(|| metadata.get("source"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    imported_at_origin = cc
                        .and_then(|m| m.get("imported_at"))
                        .or_else(|| metadata.get("imported_at"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }

                // tags can be a sequence or a comma-separated string
                if let Some(t) = map.get("tags") {
                    if let Some(seq) = t.as_sequence() {
                        tags = seq
                            .iter()
                            .filter_map(|v| v.as_str())
                            .map(strip_html)
                            .filter(|s| !s.is_empty())
                            .collect();
                    } else if let Some(s) = t.as_str() {
                        tags = s
                            .split(',')
                            .map(|s| strip_html(s.trim()))
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                }
            }
        }
    }

    // Extract readme_excerpt: skip HTML comments (<!--…-->) and heading markers,
    // take first non-empty paragraph up to 500 characters.
    let excerpt = extract_excerpt(&body);

    SkillMeta {
        version,
        tags,
        author,
        description,
        description_zh,
        origin,
        imported_at_origin,
        readme_excerpt: excerpt,
    }
}

/// Remove HTML tags from a string to prevent XSS in API output.
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

/// Extract a plain-text excerpt from markdown body: skip HTML comments and
/// heading lines, return first meaningful paragraph up to 500 chars.
fn extract_excerpt(body: &str) -> String {
    // Strip HTML comment blocks (<!-- ... -->)
    let body_no_comments = remove_html_comments(body);

    let mut result = String::new();
    for line in body_no_comments.lines() {
        let trimmed = line.trim();
        // Skip headings, blank lines, horizontal rules, code fence markers
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("---")
            || trimmed.starts_with("```")
            || trimmed.starts_with("~~~")
        {
            if !result.is_empty() {
                // We already have some content — a blank line ends the paragraph
                break;
            }
            continue;
        }
        // Strip leading markdown list markers
        let text = trimmed
            .trim_start_matches("- ")
            .trim_start_matches("* ")
            .trim_start_matches("> ");
        let cleaned = strip_html(text);
        if cleaned.is_empty() {
            continue;
        }
        if !result.is_empty() {
            result.push(' ');
        }
        result.push_str(&cleaned);
        if result.len() >= 500 {
            break;
        }
    }

    if result.len() > 500 {
        // Truncate at word boundary near 500
        let truncated = &result[..500];
        if let Some(last_space) = truncated.rfind(' ') {
            format!("{}…", &truncated[..last_space])
        } else {
            format!("{}…", truncated)
        }
    } else {
        result
    }
}

/// Remove HTML comment blocks from a string.
fn remove_html_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        if let Some(end) = rest[start..].find("-->") {
            rest = &rest[start + end + 3..];
        } else {
            // Unclosed comment — drop the rest
            break;
        }
    }
    out.push_str(rest);
    out
}

/// Compute total file size and count for a skill directory (non-recursive depth limit = 3).
fn skill_dir_stats(dir: &std::path::Path) -> (u64, u32) {
    fn walk(path: &std::path::Path, depth: u32, size: &mut u64, count: &mut u32) {
        if depth > 3 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_file() {
                *count += 1;
                if let Ok(meta) = entry.metadata() {
                    *size += meta.len();
                }
            } else if ft.is_dir() {
                walk(&entry.path(), depth + 1, size, count);
            }
        }
    }
    let mut size = 0u64;
    let mut count = 0u32;
    walk(dir, 0, &mut size, &mut count);
    (size, count)
}

/// Derive an explicit trust level string from the classified source.
/// "builtin" → "high" | "hub" → "medium" | "local" → "low"
fn trust_level_from_source(source: &str) -> String {
    match source {
        "builtin" => "high".to_string(),
        "hub" => "medium".to_string(),
        _ => "low".to_string(),
    }
}

/// Marker file path for filesystem-installed skills. Presence = disabled,
/// absence = enabled. Lives under `<hub_base>/disabled/<name>` so it travels
/// with the install root and is easy to audit by `ls`.
fn skill_disabled_marker_path(
    hub: &cyberclaw_skill_runtime::skill_hub::SkillHub,
    name: &str,
) -> std::path::PathBuf {
    hub.base_dir().join("disabled").join(name)
}

fn is_filesystem_skill_disabled(
    hub: &cyberclaw_skill_runtime::skill_hub::SkillHub,
    name: &str,
) -> bool {
    skill_disabled_marker_path(hub, name).exists()
}

fn set_filesystem_skill_disabled(
    hub: &cyberclaw_skill_runtime::skill_hub::SkillHub,
    name: &str,
    disabled: bool,
) -> std::io::Result<()> {
    let path = skill_disabled_marker_path(hub, name);
    if disabled {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, b"")?;
    } else if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Cheap heuristic category classification from the skill name. The
/// admin SPA groups skills by this string so the same categories from
/// `MOCK.skills` (Development / Research / Productivity / Creative /
/// Agents / Other) surface on the UI.
///
/// Public so `chat.rs` auto-route can mirror into admin_store.
pub fn infer_category_pub(name: &str) -> String {
    infer_category(name)
}

fn infer_category(name: &str) -> String {
    let n = name.to_lowercase();
    if n.contains("review") || n.contains("test") || n.contains("migration") || n.contains("code") {
        "Development".to_string()
    } else if n.contains("research") || n.contains("search") || n.contains("literature") {
        "Research".to_string()
    } else if n.contains("calendar") || n.contains("inbox") || n.contains("mail") {
        "Productivity".to_string()
    } else if n.contains("copy") || n.contains("illustration") || n.contains("writing") {
        "Creative".to_string()
    } else if n.contains("orchestrator") || n.contains("memory") || n.contains("agent") {
        "Agents".to_string()
    } else {
        "Other".to_string()
    }
}

/// `GET /api/v1/skills` — list installed skills from the hub's lock file.
///
/// Sprint 8 Phase A: falls back to the admin_store seed list when the
/// hub has nothing installed so the SPA has content to render.
async fn list_skills(State(state): State<Arc<AppState>>) -> Json<Vec<SkillView>> {
    let hub = state.skill_hub.read().await;
    let installed = hub.list_installed();
    let hub_base = hub.base_dir().to_path_buf();
    let mut views: Vec<SkillView> = installed
        .iter()
        .map(|b| {
            let enabled = !is_filesystem_skill_disabled(&hub, &b.name);
            to_admin_view_with_base(
                b,
                Some(installed_at_for(&hub, &b.name)),
                enabled,
                Some(&hub_base),
            )
        })
        .collect();

    if views.is_empty() {
        let seeded = state.admin_store.skills.read().await;
        views = seeded
            .iter()
            .map(|s| {
                // For seeded (demo) skills, attempt enrichment from the hub base if available.
                let bundle = cyberclaw_skill_runtime::skill_hub::SkillBundle {
                    name: s.name.clone(),
                    version: String::new(),
                    description: s.description.clone(),
                    source: s.source.clone(),
                    trust_level: cyberclaw_skill_runtime::skill_scanner::SkillTrustLevel::Community,
                    sha256: None,
                    signature: None,
                    publisher_fingerprint: None,
                };
                to_admin_view_with_base(
                    &bundle,
                    Some(s.installed_at.clone()),
                    s.enabled,
                    Some(&hub_base),
                )
            })
            .collect();
    }
    Json(views)
}

/// Read the installed-at timestamp out of the hub's lock file by name.
/// Returns an empty string if not found or the lock file can't be read —
/// the admin UI renders "—" in that case.
fn installed_at_for(hub: &cyberclaw_skill_runtime::skill_hub::SkillHub, name: &str) -> String {
    let lock_path = hub.base_dir().join("installed.lock");
    let Ok(s) = std::fs::read_to_string(&lock_path) else {
        return String::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&s) else {
        return String::new();
    };
    json.get("installed")
        .and_then(|v| v.get(name))
        .and_then(|v| v.get("installed_at"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// `POST /api/v1/skills/install` — download, scan, and install.
async fn install_skill(
    State(state): State<Arc<AppState>>,
    claims: Option<axum::extract::Extension<Claims>>,
    Json(req): Json<InstallSkillRequest>,
) -> Result<(StatusCode, Json<SkillView>), ApiError> {
    // L7 — server-side role enforcement. When JWT is present, require admin.
    if let Some(axum::extract::Extension(ref c)) = claims {
        if let Err(err) = require_admin(c).await {
            audit_role_denied(state.as_ref(), c.sub.as_str(), "skill.install:role_denied").await;
            return Err(err);
        }
    }

    let name = req.name.trim();
    let actor = claims
        .map(|c| c.0.sub.as_str().to_string())
        .unwrap_or_else(|| "system".to_string());
    if name.is_empty() {
        if let Some(sink) = state.audit.as_ref() {
            sink.record(AuditEntry::now(
                actor.clone(),
                AuditKind::Mutation,
                "skill.install".to_string(),
                None,
                serde_json::json!({}),
                AuditResult::Failure {
                    reason: "name is required".to_string(),
                },
            ))
            .await;
        }
        return Err(ApiError::InvalidRequest("name is required".to_string()));
    }

    let mut hub = state.skill_hub.write().await;

    // If a local_path is provided, register it as a source so the hub
    // can copy the skill directory into quarantine. Without a source,
    // we still allow "registering" the bundle metadata so the admin UI
    // sees an entry; the scanner path below will simply report NotFound.
    if let Some(local_path) = req.local_path.as_ref() {
        hub.add_source(SkillSource::Local {
            path: std::path::PathBuf::from(local_path),
        });
    }

    let bundle = SkillBundle {
        name: name.to_string(),
        version: req.version.clone(),
        description: req.description.clone(),
        source: req.source.clone(),
        trust_level: SkillTrustLevel::Community,
        sha256: None,
        signature: None,
        publisher_fingerprint: None,
    };
    hub.register_bundle(bundle.clone());

    // Download → scan → install.
    let scanner = SkillScanner::new();
    match hub.download(&bundle) {
        Ok(_) => match hub.scan_and_install(&bundle, &scanner) {
            Ok(SkillState::Installed) => {
                info!(skill = %name, "skill install verdict: Installed");
            }
            Ok(SkillState::Available) | Ok(SkillState::Quarantined) => {
                // scan_and_install never returns these in current
                // skill-runtime (always Installed or Rejected). Treat
                // the same as Rejected for safety.
                warn!(skill = %name, "scan returned unexpected non-Installed state");
                return Err(ApiError::InternalError(format!(
                    "scan_and_install returned unexpected state for '{}'",
                    name
                )));
            }
            Ok(SkillState::Rejected) => {
                // W1 fix (2026-05-06): scanner placed the skill in
                // quarantine because findings exceeded the trust matrix
                // threshold. Previously the handler returned 201 anyway,
                // hiding the fact that nothing was actually installed.
                // Return 422 with a clear message so the caller knows
                // the skill is unavailable and an admin must promote it
                // manually (or accept the risk by uninstalling first).
                warn!(skill = %name, "scan rejected — skill placed in quarantine");
                if let Some(sink) = state.audit.as_ref() {
                    sink.record(AuditEntry::now(
                        actor.clone(),
                        AuditKind::Mutation,
                        "skill.install".to_string(),
                        Some(format!("skill:{}", name)),
                        serde_json::json!({ "verdict": "rejected" }),
                        AuditResult::Failure {
                            reason: "scanner verdict: quarantined".to_string(),
                        },
                    ))
                    .await;
                }
                return Err(ApiError::InvalidRequest(format!(
                    "skill '{}' was quarantined by the scanner — see ~/.cyberclaw/skills/quarantine/",
                    name
                )));
            }
            Err(err) => {
                warn!(skill = %name, %err, "scan_and_install failed");
                return Err(ApiError::InternalError(format!("scan failed: {err}")));
            }
        },
        Err(err) => {
            // Without a local source, this is expected. Surface a clear
            // error so the caller can attach a `local_path`.
            warn!(skill = %name, %err, "hub download failed; cannot proceed to scan");
            return Err(ApiError::InvalidRequest(format!("download failed: {err}")));
        }
    }

    let hub_base = hub.base_dir().to_path_buf();
    let view = to_admin_view_with_base(
        &bundle,
        Some(installed_at_for(&hub, name)),
        true,
        Some(&hub_base),
    );

    // Sprint 8 Phase A — reflect install in the admin_store list.
    {
        let mut seeded = state.admin_store.skills.write().await;
        seeded.retain(|s| s.name != view.name);
        seeded.insert(
            0,
            crate::admin_store::AdminSkill {
                skill_id: view.skill_id.clone(),
                name: view.name.clone(),
                category: view.category.clone(),
                source: view.source.clone(),
                description: view.description.clone(),
                installed_at: chrono::Utc::now().to_rfc3339(),
                enabled: true,
            },
        );
    }

    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            actor,
            AuditKind::Mutation,
            "skill.install".to_string(),
            Some(format!("skill:{}", view.skill_id)),
            serde_json::json!({ "source": view.source }),
            AuditResult::Success,
        ))
        .await;
    }

    // W2 fix (2026-05-06): also populate the FTS5 skill discovery index
    // so /api/v1/skills/search?q= can find this entry. Mirrors the same
    // best-effort upsert that `create_skill` already does.
    if let Some(idx) = state.skill_index.as_ref() {
        let entry = crate::skill_index::SkillIndexEntry {
            skill_id: view.skill_id.clone(),
            name: view.name.clone(),
            description: view.description.clone(),
            category: view.category.clone(),
            source_request_id: None,
            installed_at: view.installed_at.parse::<i64>().unwrap_or(0),
        };
        if let Err(e) = idx.upsert(entry).await {
            warn!(skill = %view.name, %e, "skill index upsert failed; search will miss this entry");
        }
    }

    Ok((StatusCode::CREATED, Json(view)))
}

/// `POST /api/v1/skills/install-remote` — download + scan + install a
/// skill from a GitHub repo or a tarball URL.
///
/// Integrity: when `sha256` is provided, `SkillHub::download` verifies the
/// quarantined tree hash against the expected digest. Mismatches reject
/// the install and leave the audit trail with a `scan_failed` entry.
async fn install_skill_remote(
    State(state): State<Arc<AppState>>,
    claims: Option<axum::extract::Extension<Claims>>,
    Json(req): Json<InstallRemoteSkillRequest>,
) -> Result<(StatusCode, Json<SkillView>), ApiError> {
    // L7 — server-side role enforcement. When JWT is present, require admin.
    if let Some(axum::extract::Extension(ref c)) = claims {
        if let Err(err) = require_admin(c).await {
            audit_role_denied(
                state.as_ref(),
                c.sub.as_str(),
                "skill.install_remote:role_denied",
            )
            .await;
            return Err(err);
        }
    }

    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::InvalidRequest("name is required".to_string()));
    }
    let actor = claims
        .map(|c| c.0.sub.as_str().to_string())
        .unwrap_or_else(|| "system".to_string());

    // Translate the incoming `kind` + `url` into a typed SkillSource so the
    // hub does all the heavy lifting (git clone / curl + tar + sha256 check).
    let (source, source_label) = match req.kind.as_str() {
        "github" => {
            let (owner, repo, branch) = match parse_github_spec(&req.url) {
                Ok(v) => v,
                Err(err) => return Err(ApiError::InvalidRequest(err)),
            };
            let label = format!("github:{owner}/{repo}");
            (
                SkillSource::GitHub {
                    owner,
                    repo,
                    branch,
                },
                label,
            )
        }
        "registry" => {
            if req.url.trim().is_empty() {
                return Err(ApiError::InvalidRequest("url is required".to_string()));
            }
            let label = format!("registry:{}", req.url);
            (
                SkillSource::Registry {
                    url: req.url.clone(),
                },
                label,
            )
        }
        other => {
            return Err(ApiError::InvalidRequest(format!(
                "unsupported kind '{other}' (expected 'github' or 'registry')"
            )));
        }
    };

    let mut hub = state.skill_hub.write().await;
    hub.add_source(source);

    let bundle = SkillBundle {
        name: name.clone(),
        version: req.version.clone(),
        description: req.description.clone(),
        source: source_label.clone(),
        trust_level: SkillTrustLevel::Community,
        sha256: req.sha256.clone(),
        signature: None,
        publisher_fingerprint: None,
    };
    hub.register_bundle(bundle.clone());

    let scanner = SkillScanner::new();
    match hub.download(&bundle) {
        Ok(_) => match hub.scan_and_install(&bundle, &scanner) {
            Ok(state_tag) => {
                info!(skill = %name, ?state_tag, "remote skill install verdict");
            }
            Err(err) => {
                warn!(skill = %name, %err, "scan_and_install failed");
                audit_failure(state.as_ref(), &actor, &name, &err.to_string()).await;
                return Err(ApiError::InternalError(format!("scan failed: {err}")));
            }
        },
        Err(err) => {
            warn!(skill = %name, %err, "remote skill download failed");
            audit_failure(state.as_ref(), &actor, &name, &err.to_string()).await;
            return Err(ApiError::InvalidRequest(format!("download failed: {err}")));
        }
    }

    let hub_base = hub.base_dir().to_path_buf();
    let view = to_admin_view_with_base(
        &bundle,
        Some(installed_at_for(&hub, &name)),
        true,
        Some(&hub_base),
    );

    {
        let mut seeded = state.admin_store.skills.write().await;
        seeded.retain(|s| s.name != view.name);
        seeded.insert(
            0,
            crate::admin_store::AdminSkill {
                skill_id: view.skill_id.clone(),
                name: view.name.clone(),
                category: view.category.clone(),
                source: view.source.clone(),
                description: view.description.clone(),
                installed_at: chrono::Utc::now().to_rfc3339(),
                enabled: true,
            },
        );
    }

    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            actor,
            AuditKind::Mutation,
            "skill.install_remote".to_string(),
            Some(format!("skill:{}", view.skill_id)),
            serde_json::json!({
                "source": source_label,
                "sha256": req.sha256,
            }),
            AuditResult::Success,
        ))
        .await;
    }

    Ok((StatusCode::CREATED, Json(view)))
}

/// Parse a GitHub spec in the form `owner/repo` or `owner/repo#branch`.
/// Also accepts full URLs like `https://github.com/owner/repo` for
/// convenience — query params and `.git` suffixes are stripped.
fn parse_github_spec(raw: &str) -> Result<(String, String, Option<String>), String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("url is required".to_string());
    }
    let (spec, branch) = match raw.split_once('#') {
        Some((s, b)) if !b.is_empty() => (s, Some(b.to_string())),
        _ => (raw, None),
    };
    // Strip a full GitHub URL prefix if present.
    let spec = spec
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("github.com/")
        .trim_end_matches(".git")
        .trim_end_matches('/');
    let parts: Vec<&str> = spec.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 2 {
        return Err(format!(
            "invalid github spec '{raw}': expected 'owner/repo' or 'owner/repo#branch'"
        ));
    }
    let owner = parts[0].to_string();
    let repo = parts[1].to_string();
    Ok((owner, repo, branch))
}

/// Record a role-denied write attempt as an `Auth` audit failure. Mirrors
/// the helpers in `api::reviews` / `api::settings` so the full sensitive
/// surface logs identical role-denied provenance.
async fn audit_role_denied(state: &AppState, actor: &str, action: &str) {
    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            actor.to_string(),
            AuditKind::Auth,
            action.to_string(),
            None,
            serde_json::json!({ "reason": "role_denied" }),
            AuditResult::Failure {
                reason: "admin role required".to_string(),
            },
        ))
        .await;
    }
}

/// Record a failed remote-install attempt in the audit sink.
async fn audit_failure(state: &AppState, actor: &str, skill_name: &str, reason: &str) {
    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            actor.to_string(),
            AuditKind::Mutation,
            "skill.install_remote".to_string(),
            Some(format!("skill:{skill_name}")),
            serde_json::json!({ "reason": reason }),
            AuditResult::Failure {
                reason: reason.to_string(),
            },
        ))
        .await;
    }
}

/// `POST /api/v1/skills/create` — scaffold a new skill from a natural-language
/// description. Writes `~/.cyberclaw/skills/{name}/SKILL.md` and registers it
/// with the hub. Rejects the request when [`IntentClassifier`] does not
/// confirm a CreateSkill intent.
async fn create_skill(
    State(state): State<Arc<AppState>>,
    claims: Option<axum::extract::Extension<Claims>>,
    Json(req): Json<CreateSkillRequest>,
) -> Result<(StatusCode, Json<SkillView>), ApiError> {
    // L7 — server-side role enforcement. When JWT is present, require admin.
    if let Some(axum::extract::Extension(ref c)) = claims {
        if let Err(err) = require_admin(c).await {
            audit_role_denied(state.as_ref(), c.sub.as_str(), "skill.create:role_denied").await;
            return Err(err);
        }
    }

    let actor = claims
        .map(|c| c.0.sub.as_str().to_string())
        .unwrap_or_else(|| "system".to_string());
    let view = create_skill_core(&state, req, &actor).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

/// Sprint 21 — core skill creation logic, callable both from the HTTP
/// handler and from the agentic loop's `skill_create` tool intercept.
/// Skips the role check (caller is responsible for that) but performs all
/// other validation, scaffold writing, hub install, and audit recording.
pub async fn create_skill_core(
    state: &Arc<AppState>,
    req: CreateSkillRequest,
    actor: &str,
) -> Result<SkillView, ApiError> {
    let name = req.name.trim().to_string();

    if name.is_empty() {
        return Err(ApiError::InvalidRequest("name is required".to_string()));
    }
    // Slug guard — mirrors frontend regex ^[a-z][a-z0-9-]{2,31}$
    let valid = name.len() >= 3
        && name.len() <= 32
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        return Err(ApiError::InvalidRequest(
            "name must match ^[a-z][a-z0-9-]{2,31}$".to_string(),
        ));
    }
    if req.description.trim().is_empty() {
        return Err(ApiError::InvalidRequest(
            "description is required".to_string(),
        ));
    }

    // Intent gate: reject payloads that are clearly off-topic.
    let classifier = IntentClassifier::with_defaults();
    let matched = classifier.classify(&req.description);
    let classified = matched.as_ref().map(|m| m.intent);
    if !matches!(classified, Some(Intent::CreateSkill) | None) {
        // Matched, but not CreateSkill — reject with guidance.
        return Err(ApiError::InvalidRequest(format!(
            "description classified as {:?}; use the right endpoint",
            classified.unwrap()
        )));
    }

    // Write scaffold under ~/.cyberclaw/skills/scaffold/{name}/SKILL.md.
    //
    // SkillHub::download with `SkillSource::Local { path }` copies
    // `path/<bundle.name>` into the quarantine dir, so the source parent
    // must be the directory that CONTAINS `{name}/` — not the
    // `{name}/` itself. We use a dedicated `scaffold/` sibling of
    // `installed/` + `quarantine/` so scaffolds never collide with
    // hub-managed layouts.
    let base_dir = std::env::var_os("HOME")
        .map(|h| {
            std::path::PathBuf::from(h)
                .join(".cyberclaw")
                .join("skills")
        })
        .unwrap_or_else(|| std::path::PathBuf::from(".cyberclaw").join("skills"));
    let scaffold_root = base_dir.join("scaffold");
    let skill_dir = scaffold_root.join(&name);
    if let Err(err) = std::fs::create_dir_all(&skill_dir) {
        return Err(ApiError::InternalError(format!("create dir failed: {err}")));
    }

    let scaffold = build_skill_scaffold(&name, &req);
    let skill_md = skill_dir.join("SKILL.md");
    if let Err(err) = std::fs::write(&skill_md, &scaffold) {
        return Err(ApiError::InternalError(format!(
            "write SKILL.md failed: {err}"
        )));
    }

    // Register + install via SkillHub. Source path is the scaffold ROOT
    // (parent of `{name}/`) so `download` can resolve `{scaffold_root}/{name}`.
    let mut hub = state.skill_hub.write().await;
    hub.add_source(SkillSource::Local {
        path: scaffold_root.clone(),
    });
    let bundle = SkillBundle {
        name: name.clone(),
        version: "0.1.0".to_string(),
        description: req.description.clone(),
        source: format!("local:{}", skill_dir.display()),
        trust_level: SkillTrustLevel::Community,
        sha256: None,
        signature: None,
        publisher_fingerprint: None,
    };
    hub.register_bundle(bundle.clone());

    let scanner = SkillScanner::new();
    match hub.download(&bundle) {
        Ok(_) => match hub.scan_and_install(&bundle, &scanner) {
            Ok(state_tag) => {
                info!(skill = %name, ?state_tag, classified = ?classified, "skill scaffold installed");
            }
            Err(err) => {
                warn!(skill = %name, %err, "scan_and_install failed");
                return Err(ApiError::InternalError(format!("scan failed: {err}")));
            }
        },
        Err(err) => {
            warn!(skill = %name, %err, "hub download failed");
            return Err(ApiError::InternalError(format!(
                "hub download failed: {err}"
            )));
        }
    }

    let hub_base = hub.base_dir().to_path_buf();
    let view = to_admin_view_with_base(
        &bundle,
        Some(installed_at_for(&hub, &name)),
        true,
        Some(&hub_base),
    );

    // Mirror into admin_store so the Skills page sees it immediately.
    {
        let mut seeded = state.admin_store.skills.write().await;
        seeded.retain(|s| s.name != view.name);
        seeded.insert(
            0,
            crate::admin_store::AdminSkill {
                skill_id: view.skill_id.clone(),
                name: view.name.clone(),
                category: view.category.clone(),
                source: view.source.clone(),
                description: view.description.clone(),
                installed_at: view.installed_at.clone(),
                enabled: true,
            },
        );
    }

    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            actor.to_string(),
            AuditKind::Mutation,
            "skill.create".to_string(),
            Some(view.skill_id.clone()),
            serde_json::json!({
                "name": view.name,
                "classified_intent": classified.map(|i| format!("{:?}", i)),
                "source_request_id": req.source_request_id,
            }),
            AuditResult::Success,
        ))
        .await;
    }

    // Sprint 21 path #3 — populate the FTS5 skill discovery index so
    // future agents can `skill_search` for this skill before creating
    // duplicates. Best-effort: index failure is logged but does not
    // fail the create call.
    if let Some(idx) = state.skill_index.as_ref() {
        let entry = crate::skill_index::SkillIndexEntry {
            skill_id: view.skill_id.clone(),
            name: view.name.clone(),
            description: view.description.clone(),
            category: view.category.clone(),
            source_request_id: req.source_request_id.clone(),
            installed_at: chrono::DateTime::parse_from_rfc3339(&view.installed_at)
                .map(|d| d.timestamp())
                .unwrap_or_else(|_| view.installed_at.parse::<i64>().unwrap_or(0)),
        };
        if let Err(e) = idx.upsert(entry).await {
            warn!(skill = %view.name, %e, "skill index upsert failed; search will miss this entry");
        }
    }

    Ok(view)
}

/// `GET /api/v1/skills/search` query params.
#[derive(Debug, Deserialize)]
pub struct SkillSearchQuery {
    pub q: String,
    #[serde(default = "default_search_limit")]
    pub limit: u32,
}

fn default_search_limit() -> u32 {
    20
}

/// `GET /api/v1/skills/search` — FTS5 search over installed skills.
///
/// Empty `q` returns the most-recently-indexed skills. Otherwise runs
/// a phrase MATCH against name/description/category.
async fn search_skills(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<SkillSearchQuery>,
) -> Result<Json<Vec<crate::skill_index::SkillIndexEntry>>, ApiError> {
    let idx = state.skill_index.as_ref().ok_or_else(|| {
        ApiError::NotFound("skill index not initialized on this deployment".to_string())
    })?;
    let results = idx
        .search(params.q, params.limit)
        .await
        .map_err(|e| ApiError::InternalError(format!("skill search failed: {e}")))?;
    Ok(Json(results))
}

/// Build the initial `SKILL.md` body — public shim for `chat.rs` auto-route.
pub fn build_skill_scaffold_pub(name: &str, req: &CreateSkillRequest) -> String {
    build_skill_scaffold(name, req)
}

/// Build the initial `SKILL.md` body for [`create_skill`].
fn build_skill_scaffold(name: &str, req: &CreateSkillRequest) -> String {
    let now = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let methodology = req.methodology.as_deref().unwrap_or(
        "> TODO(human): 填入方法论主体。描述触发条件、使用步骤、反模式。\n\
         参考 `ecosystem/skills/brainstorm/SKILL.md` 的结构。",
    );
    let examples = if req.trigger_examples.is_empty() {
        "_（留空，后续通过 skill-creator 生成 eval 集）_".to_string()
    } else {
        req.trigger_examples
            .iter()
            .map(|e| format!("- {e}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let provenance_line = req
        .source_request_id
        .as_deref()
        .map(|rid| format!("source_request_id: {rid}\n"))
        .unwrap_or_default();
    format!(
        "---\n\
         name: {name}\n\
         description: {description}\n\
         source: cyberclaw (created via /api/v1/skills/create, {now})\n\
         {provenance_line}\
         level: 2\n\
         status: scaffold\n\
         ---\n\
         \n\
         <!--\n\
         CyberClaw adaptation notes:\n\
         \n\
         - 本 Skill 由 `/api/v1/skills/create` 创建，是一个 scaffold。\n\
         - 真正的方法论优化（description tuning / trigger eval）通过\n\
           `crates/cyberclaw-control-plane/src/skill_creator.rs` facade\n\
           驱动，内部调用 EvolutionOrchestrator。\n\
         - 本 Skill 不执行任何代码，不 spawn 子进程，不调用 HTTP。\n\
         -->\n\
         \n\
         # {name} — {description}\n\
         \n\
         ## Methodology\n\
         \n\
         {methodology}\n\
         \n\
         ## Trigger Examples\n\
         \n\
         {examples}\n\
         \n\
         ## Anti-Patterns\n\
         \n\
         > TODO(human): 列出 **不** 该触发本 Skill 的场景。\n",
        name = name,
        description = req.description,
        now = now,
        methodology = methodology,
        examples = examples,
    )
}

// ---------------------------------------------------------------------------
// Registry listing
// ---------------------------------------------------------------------------

/// `GET /api/v1/skills/registry` — list all bundles known to the hub
/// (installed + discovered-but-not-yet-installed).
///
/// Returns the same `SkillView` shape as `GET /api/v1/skills` so the
/// admin UI can use a single component for both local and catalog views.
async fn list_registry(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let hub = state.skill_hub.read().await;
    let hub_base = hub.base_dir().to_path_buf();
    let bundles = hub.search("");
    let views: Vec<SkillView> = bundles
        .iter()
        .map(|b| to_admin_view_with_base(b, None, true, Some(&hub_base)))
        .collect();
    let total = views.len();
    Ok(Json(
        serde_json::json!({ "bundles": views, "total": total }),
    ))
}

// ---------------------------------------------------------------------------
// Publisher key ring
// ---------------------------------------------------------------------------

/// `GET /api/v1/skills/publisher-keys` — list fingerprints in the publisher ring.
async fn list_publisher_keys(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ring = state.publisher_ring.read().await;
    let fingerprints: Vec<String> = ring.fingerprints().iter().map(|s| s.to_string()).collect();
    let count = fingerprints.len();
    Ok(Json(
        serde_json::json!({ "fingerprints": fingerprints, "count": count }),
    ))
}

/// Request body for `POST /api/v1/skills/publisher-keys`.
#[derive(Debug, Deserialize)]
struct AddPublisherKeyRequest {
    /// ASCII-armored OpenPGP public key (`-----BEGIN PGP PUBLIC KEY BLOCK-----…`).
    armored_key: String,
}

/// `POST /api/v1/skills/publisher-keys` — add a trusted publisher key (admin only).
///
/// Returns the fingerprint of the newly added key.
async fn add_publisher_key(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<AddPublisherKeyRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&claims)
        .await
        .map_err(|_| ApiError::Forbidden("admin role required".to_string()))?;

    let fingerprint = {
        let mut ring = state.publisher_ring.write().await;
        ring.add_armored_key(&req.armored_key)
            .map_err(|e| ApiError::InvalidRequest(format!("invalid key: {e}")))?
    };

    // Persist to disk so the ring survives restarts.
    {
        let ring = state.publisher_ring.read().await;
        let ring_path = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".cyberclaw/trust/publishers.asc");
        if let Err(e) = ring.save_to_file(&ring_path) {
            warn!(path = %ring_path.display(), error = %e, "failed to persist publisher ring");
        }
    }

    info!(fingerprint = %fingerprint, "publisher key added");
    Ok(Json(
        serde_json::json!({ "fingerprint": fingerprint, "status": "added" }),
    ))
}

/// `GET /api/v1/skills/:id/content` — return the `SKILL.md` body.
async fn get_skill_content(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SkillContentResponse>, ApiError> {
    let hub = state.skill_hub.read().await;

    // Accept both the raw name and the `sk_<name>` admin id.
    let name = id.strip_prefix("sk_").unwrap_or(&id);
    let skill_md = hub.base_dir().join("installed").join(name).join("SKILL.md");

    match std::fs::read_to_string(&skill_md) {
        Ok(content) => Ok(Json(SkillContentResponse {
            skill_id: id,
            content,
        })),
        Err(err) => {
            warn!(path = %skill_md.display(), %err, "SKILL.md not readable");
            Err(ApiError::NotFound(format!(
                "skill '{name}' has no readable SKILL.md"
            )))
        }
    }
}

/// `POST /api/v1/skills/:id/toggle` body.
#[derive(Debug, Deserialize)]
struct ToggleSkillRequest {
    enabled: bool,
}

/// `POST /api/v1/skills/:id/toggle` — enable or disable a skill (in-memory).
async fn toggle_skill(
    State(state): State<Arc<AppState>>,
    claims: Option<axum::extract::Extension<Claims>>,
    Path(id): Path<String>,
    Json(req): Json<ToggleSkillRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if let Some(axum::extract::Extension(ref c)) = claims {
        if let Err(err) = require_admin(c).await {
            audit_role_denied(state.as_ref(), c.sub.as_str(), "skill.toggle:role_denied").await;
            return Err(err);
        }
    }

    let actor = claims
        .map(|c| c.0.sub.as_str().to_string())
        .unwrap_or_else(|| "system".to_string());

    let name = id.strip_prefix("sk_").unwrap_or(&id).to_string();
    let mut seeded = state.admin_store.skills.write().await;
    let skill = seeded
        .iter_mut()
        .find(|s| s.name == name || s.skill_id == id);
    if let Some(s) = skill {
        s.enabled = req.enabled;
        let view = serde_json::json!({
            "skill_id": s.skill_id,
            "name": s.name,
            "enabled": s.enabled,
        });
        drop(seeded);
        if let Some(sink) = state.audit.as_ref() {
            sink.record(AuditEntry::now(
                actor,
                AuditKind::Mutation,
                "skill.toggle".to_string(),
                Some(format!("skill:{id}")),
                serde_json::json!({ "enabled": req.enabled, "source": "admin_store" }),
                AuditResult::Success,
            ))
            .await;
        }
        return Ok(Json(view));
    }
    drop(seeded);

    // Fall through: try the filesystem-installed skill set. Presence of a
    // bundle named `name` means we can record a `disabled` marker — toggling
    // is purely a marker write under `<hub>/disabled/<name>`.
    let hub = state.skill_hub.read().await;
    let bundle_exists = hub.list_installed().iter().any(|b| b.name == name);
    if !bundle_exists {
        return Err(ApiError::NotFound(format!("skill not found: {id}")));
    }
    set_filesystem_skill_disabled(&hub, &name, !req.enabled)
        .map_err(|e| ApiError::InternalError(format!("write disabled marker: {e}")))?;
    let view = serde_json::json!({
        "skill_id": id,
        "name": name,
        "enabled": req.enabled,
    });
    drop(hub);
    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            actor,
            AuditKind::Mutation,
            "skill.toggle".to_string(),
            Some(format!("skill:{id}")),
            serde_json::json!({ "enabled": req.enabled, "source": "filesystem" }),
            AuditResult::Success,
        ))
        .await;
    }
    Ok(Json(view))
}

/// `DELETE /api/v1/skills/:id` — uninstall.
async fn uninstall_skill(
    State(state): State<Arc<AppState>>,
    claims: Option<axum::extract::Extension<Claims>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // L7 — server-side role enforcement. When JWT is present, require admin.
    if let Some(axum::extract::Extension(ref c)) = claims {
        if let Err(err) = require_admin(c).await {
            audit_role_denied(
                state.as_ref(),
                c.sub.as_str(),
                "skill.uninstall:role_denied",
            )
            .await;
            return Err(err);
        }
    }

    let name = id.strip_prefix("sk_").unwrap_or(&id).to_string();
    let actor = claims
        .map(|c| c.0.sub.as_str().to_string())
        .unwrap_or_else(|| "system".to_string());

    // Try the hub first; if it's not installed there, fall through to the
    // admin_store seed list so the UI's delete-demo-skill action works.
    let hub_result = {
        let mut hub = state.skill_hub.write().await;
        hub.remove(&name)
    };

    let outcome: Result<StatusCode, ApiError> = match hub_result {
        Ok(()) | Err(cyberclaw_skill_runtime::skill_hub::HubError::NotFound(_)) => {
            let mut seeded = state.admin_store.skills.write().await;
            let before = seeded.len();
            seeded.retain(|s| s.name != name && s.skill_id != id);
            if before == seeded.len() && hub_result.is_err() {
                Err(ApiError::NotFound(format!("skill not installed: {name}")))
            } else {
                Ok(StatusCode::NO_CONTENT)
            }
        }
        Err(err) => Err(ApiError::InternalError(format!("remove failed: {err}"))),
    };

    if let Some(sink) = state.audit.as_ref() {
        let result = match &outcome {
            Ok(_) => AuditResult::Success,
            Err(e) => AuditResult::Failure {
                reason: e.to_string(),
            },
        };
        sink.record(AuditEntry::now(
            actor,
            AuditKind::Mutation,
            "skill.uninstall".to_string(),
            Some(format!("skill:{}", id)),
            serde_json::json!({}),
            result,
        ))
        .await;
    }

    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_source_maps_known_prefixes() {
        assert_eq!(classify_source("builtin"), "builtin");
        assert_eq!(classify_source("builtin:core"), "builtin");
        assert_eq!(classify_source("github:owner/repo"), "hub");
        assert_eq!(classify_source("registry:https://..."), "hub");
        assert_eq!(classify_source("ecosystem:brainstorm"), "hub");
        assert_eq!(classify_source("ecosystem:architecture-diagram"), "hub");
        assert_eq!(classify_source("hub"), "hub");
        assert_eq!(classify_source("local"), "local");
        assert_eq!(classify_source("/tmp/some/path"), "local");
    }

    #[test]
    fn infer_category_groups_names() {
        assert_eq!(infer_category("pull-request-review"), "Development");
        assert_eq!(infer_category("web-research-synthesizer"), "Research");
        assert_eq!(infer_category("calendar-scheduler"), "Productivity");
        assert_eq!(infer_category("copywriting-assistant"), "Creative");
        assert_eq!(infer_category("agent-orchestrator"), "Agents");
        assert_eq!(infer_category("xyz"), "Other");
    }

    #[test]
    fn to_admin_view_shapes_bundle() {
        let bundle = SkillBundle {
            name: "unit-test-authoring".to_string(),
            version: "1.0.0".to_string(),
            description: "Generates unit tests".to_string(),
            source: "builtin".to_string(),
            trust_level: SkillTrustLevel::Builtin,
            sha256: None,
            signature: None,
            publisher_fingerprint: None,
        };
        let view = to_admin_view(&bundle, Some("2026-04-18T00:00:00Z".to_string()), true);
        assert_eq!(view.skill_id, "sk_unit-test-authoring");
        assert_eq!(view.category, "Development");
        assert_eq!(view.source, "builtin");
        assert_eq!(view.installed_at, "2026-04-18T00:00:00Z");
    }

    #[tokio::test]
    async fn list_skills_router_returns_200() {
        use crate::api::test_helpers::build_test_state;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let state = build_test_state();
        let app = create_skills_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/skills")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // Empty or pre-populated, but must be a JSON array.
        assert!(json.is_array());
    }

    #[tokio::test]
    async fn install_skill_rejects_missing_name() {
        use crate::api::test_helpers::build_test_state;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let state = build_test_state();
        let app = create_skills_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/skills/install")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name": ""}"#.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ----- L7: server-side role enforcement -----

    use crate::api::test_helpers::seed_users_with_role;
    use axum::extract::{Extension, State};
    use cyberclaw_core::ids::UserId;
    use serial_test::serial;

    fn claims_for(user_id: &str) -> Claims {
        let uid = UserId::from_string(user_id.to_string()).expect("valid user id");
        let now = chrono::Utc::now().timestamp();
        Claims {
            sub: uid,
            tenant: None,
            iat: now,
            exp: now + 3600,
        }
    }

    /// `install_skill` happens to fail with `BadRequest` for the admin path
    /// when `local_path` is missing (because the hub can't download) — but
    /// crucially, it does NOT fail with `Forbidden`. That distinction is
    /// what the admin test asserts.
    #[tokio::test]
    #[serial]
    async fn test_skills_install_allowed_for_admin() {
        let (_tmp, _restore) = seed_users_with_role("op-sk-install-admin", "admin");
        let state = crate::api::test_helpers::build_test_state();
        let claims = claims_for("op-sk-install-admin");
        let res = install_skill(
            State(state),
            Some(Extension(claims)),
            axum::Json(InstallSkillRequest {
                name: "l7-test-skill".to_string(),
                version: "0.0.0".to_string(),
                description: "".to_string(),
                source: "local".to_string(),
                local_path: None,
            }),
        )
        .await;
        if let Err(err) = res {
            assert!(
                !matches!(err, ApiError::Forbidden(_)),
                "admin must NOT be forbidden, got: {:?}",
                err
            );
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_skills_install_rejected_for_viewer() {
        let (_tmp, _restore) = seed_users_with_role("op-sk-install-viewer", "viewer");
        let state = crate::api::test_helpers::build_test_state();
        let claims = claims_for("op-sk-install-viewer");
        let err = install_skill(
            State(state),
            Some(Extension(claims)),
            axum::Json(InstallSkillRequest {
                name: "l7-test-viewer".to_string(),
                version: "0.0.0".to_string(),
                description: "".to_string(),
                source: "local".to_string(),
                local_path: None,
            }),
        )
        .await
        .expect_err("viewer must be forbidden");
        assert!(matches!(err, ApiError::Forbidden(_)), "got {:?}", err);
    }

    #[tokio::test]
    #[serial]
    async fn test_skills_install_remote_allowed_for_admin() {
        let (_tmp, _restore) = seed_users_with_role("op-sk-remote-admin", "admin");
        let state = crate::api::test_helpers::build_test_state();
        let claims = claims_for("op-sk-remote-admin");
        let res = install_skill_remote(
            State(state),
            Some(Extension(claims)),
            axum::Json(InstallRemoteSkillRequest {
                name: "l7-remote-admin".to_string(),
                kind: "github".to_string(),
                url: "invalid-spec".to_string(),
                version: "0.0.0".to_string(),
                description: "".to_string(),
                sha256: None,
            }),
        )
        .await;
        if let Err(err) = res {
            assert!(
                !matches!(err, ApiError::Forbidden(_)),
                "admin must NOT be forbidden, got: {:?}",
                err
            );
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_skills_install_remote_rejected_for_viewer() {
        let (_tmp, _restore) = seed_users_with_role("op-sk-remote-viewer", "viewer");
        let state = crate::api::test_helpers::build_test_state();
        let claims = claims_for("op-sk-remote-viewer");
        let err = install_skill_remote(
            State(state),
            Some(Extension(claims)),
            axum::Json(InstallRemoteSkillRequest {
                name: "l7-remote-viewer".to_string(),
                kind: "github".to_string(),
                url: "owner/repo".to_string(),
                version: "0.0.0".to_string(),
                description: "".to_string(),
                sha256: None,
            }),
        )
        .await
        .expect_err("viewer must be forbidden");
        assert!(matches!(err, ApiError::Forbidden(_)), "got {:?}", err);
    }

    #[tokio::test]
    #[serial]
    async fn test_skills_create_allowed_for_admin() {
        let (_tmp, _restore) = seed_users_with_role("op-sk-create-admin", "admin");
        let state = crate::api::test_helpers::build_test_state();
        let claims = claims_for("op-sk-create-admin");
        // Intentionally blank description to avoid actually creating a
        // scaffold on disk — this fails at the validation stage, not the
        // role gate. We only care that it is NOT Forbidden.
        let res = create_skill(
            State(state),
            Some(Extension(claims)),
            axum::Json(CreateSkillRequest {
                name: "l7-create-admin".to_string(),
                description: "".to_string(),
                methodology: None,
                trigger_examples: vec![],
                source_request_id: None,
            }),
        )
        .await;
        if let Err(err) = res {
            assert!(
                !matches!(err, ApiError::Forbidden(_)),
                "admin must NOT be forbidden, got: {:?}",
                err
            );
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_skills_create_rejected_for_viewer() {
        let (_tmp, _restore) = seed_users_with_role("op-sk-create-viewer", "viewer");
        let state = crate::api::test_helpers::build_test_state();
        let claims = claims_for("op-sk-create-viewer");
        let err = create_skill(
            State(state),
            Some(Extension(claims)),
            axum::Json(CreateSkillRequest {
                name: "l7-create-viewer".to_string(),
                description: "create a new skill".to_string(),
                methodology: None,
                trigger_examples: vec![],
                source_request_id: None,
            }),
        )
        .await
        .expect_err("viewer must be forbidden");
        assert!(matches!(err, ApiError::Forbidden(_)), "got {:?}", err);
    }

    #[tokio::test]
    #[serial]
    async fn test_skills_uninstall_allowed_for_admin() {
        let (_tmp, _restore) = seed_users_with_role("op-sk-uninstall-admin", "admin");
        let state = crate::api::test_helpers::build_test_state();
        let claims = claims_for("op-sk-uninstall-admin");
        let res = uninstall_skill(
            State(state),
            Some(Extension(claims)),
            axum::extract::Path("sk_does-not-exist".to_string()),
        )
        .await;
        // Missing skill returns NotFound for admin — NEVER Forbidden.
        if let Err(err) = res {
            assert!(
                !matches!(err, ApiError::Forbidden(_)),
                "admin must NOT be forbidden, got: {:?}",
                err
            );
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_skills_uninstall_rejected_for_viewer() {
        let (_tmp, _restore) = seed_users_with_role("op-sk-uninstall-viewer", "viewer");
        let state = crate::api::test_helpers::build_test_state();
        let claims = claims_for("op-sk-uninstall-viewer");
        let err = uninstall_skill(
            State(state),
            Some(Extension(claims)),
            axum::extract::Path("sk_any".to_string()),
        )
        .await
        .expect_err("viewer must be forbidden");
        assert!(matches!(err, ApiError::Forbidden(_)), "got {:?}", err);
    }
}
