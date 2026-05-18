//! Multipart upload endpoint — receives binary files (images / audio /
//! arbitrary blobs) and persists them under `<workspace>/uploads/`.
//!
//! Decouples upload (multipart wire format) from capability dispatch
//! (path-based input). After upload the client receives a server path
//! that vision.analyze_image / audio.transcribe / file_read / etc.
//! can consume via their existing JSON input schema.
//!
//! Limits:
//!   · 25 MB per file (axum body-size limit applies upstream)
//!   · workspace boundary enforced — uploads sandboxed to /uploads/
//!   · filename sanitized (no traversal, ASCII subset)
//!
//! Audit emit:
//!   · `upload.create` on success — actor / path / size / mime
//!   · `upload.create:role_denied` on auth failure

use axum::{
    extract::{Multipart, Path as AxumPath, State},
    routing::{delete as delete_method, get, post},
    Extension, Json, Router,
};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::Claims;
use crate::state::AppState;

const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;
const UPLOADS_SUBDIR: &str = "uploads";

#[derive(Debug, Serialize)]
pub struct UploadResponse {
    pub id: String,
    pub path: String,
    pub size_bytes: usize,
    pub mime: String,
    pub original_filename: Option<String>,
}

pub fn create_uploads_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/uploads", post(upload_file))
        .route("/api/v1/uploads", get(list_uploads))
        .route("/api/v1/uploads/:id", delete_method(delete_upload))
}

/// Single entry in the list response — mirrors the upload-time
/// [`UploadResponse`] minus mime-sniff (re-sniffing every list entry would
/// be wasteful; clients that need mime can re-read the file or rely on
/// the original POST response).
#[derive(Debug, Serialize)]
pub struct UploadListEntry {
    pub id: String,
    pub stored_filename: String,
    pub path: String,
    pub size_bytes: u64,
    pub created_at: Option<String>, // RFC3339 from filesystem mtime
}

#[derive(Debug, Serialize)]
pub struct UploadListResponse {
    pub uploads: Vec<UploadListEntry>,
    pub total: usize,
}

/// `GET /api/v1/uploads` — list everything in the uploads dir. No
/// per-actor filtering yet (single-tenant assumption); upload IDs are
/// UUID prefixes on the stored filename.
async fn list_uploads(
    State(_state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
) -> Result<Json<UploadListResponse>, ApiError> {
    let dir = uploads_dir();
    if !dir.exists() {
        return Ok(Json(UploadListResponse {
            uploads: Vec::new(),
            total: 0,
        }));
    }
    let mut rd = tokio::fs::read_dir(&dir)
        .await
        .map_err(|e| ApiError::InternalError(format!("read uploads dir: {e}")))?;
    let mut entries = Vec::new();
    while let Some(ent) = rd
        .next_entry()
        .await
        .map_err(|e| ApiError::InternalError(format!("read dir entry: {e}")))?
    {
        let meta = match ent.metadata().await {
            Ok(m) => m,
            Err(_) => continue, // skip on transient stat error
        };
        if !meta.is_file() {
            continue;
        }
        let stored = ent.file_name().to_string_lossy().to_string();
        // Filename layout: "{uuid}-{safe_name}". Split on first '-'.
        let id = stored
            .split_once('-')
            .map(|(uuid, _)| uuid.to_string())
            .unwrap_or_else(|| stored.clone());
        let created_at = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| {
                chrono::DateTime::<chrono::Utc>::from_timestamp(
                    d.as_secs() as i64,
                    d.subsec_nanos(),
                )
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()
            });
        entries.push(UploadListEntry {
            id,
            stored_filename: stored.clone(),
            path: ent.path().to_string_lossy().to_string(),
            size_bytes: meta.len(),
            created_at,
        });
    }
    // Newest first by created_at
    entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let total = entries.len();
    Ok(Json(UploadListResponse {
        uploads: entries,
        total,
    }))
}

/// `DELETE /api/v1/uploads/:id` — remove one upload by ID. ID is matched
/// as a filename prefix (the UUID portion of `{uuid}-{name}`).
async fn delete_upload(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Reject suspicious IDs to defense-in-depth against traversal — UUIDs
    // are hex + dashes only, never `..` or `/`.
    if id.contains("..") || id.contains('/') || id.contains('\\') || id.is_empty() {
        return Err(ApiError::InvalidRequest("invalid upload id".to_string()));
    }
    let dir = uploads_dir();
    let mut rd = tokio::fs::read_dir(&dir)
        .await
        .map_err(|e| ApiError::InternalError(format!("read uploads dir: {e}")))?;
    let mut deleted: Option<String> = None;
    while let Some(ent) = rd
        .next_entry()
        .await
        .map_err(|e| ApiError::InternalError(format!("read dir entry: {e}")))?
    {
        let name = ent.file_name().to_string_lossy().to_string();
        if name.starts_with(&format!("{id}-")) || name == id {
            let p = ent.path();
            if !path_is_within(&p, &dir) {
                continue; // belt-and-suspenders
            }
            tokio::fs::remove_file(&p)
                .await
                .map_err(|e| ApiError::InternalError(format!("delete: {e}")))?;
            deleted = Some(name);
            break;
        }
    }
    let actor = claims.sub.to_string();
    match deleted {
        Some(name) => {
            if let Some(sink) = state.audit.as_ref() {
                sink.record(AuditEntry::now(
                    actor,
                    AuditKind::Mutation,
                    "upload.delete".to_string(),
                    Some(format!("upload:{id}")),
                    serde_json::json!({ "id": id, "filename": name }),
                    AuditResult::Success,
                ))
                .await;
            }
            Ok(Json(serde_json::json!({ "ok": true, "deleted": id })))
        }
        None => Err(ApiError::NotFound(format!("upload {id} not found"))),
    }
}

/// Sanitize a filename: keep ASCII alphanumerics + `.`/`-`/`_`, drop
/// everything else (no `/`, no `..`, no control chars). Empty filename
/// is replaced with `blob`.
fn sanitize_filename(raw: Option<&str>) -> String {
    // Two-pass sanitize: first remove path-traversal tokens (".."), then strip
    // any remaining non-allowlist chars. Single-pass char filter would mangle
    // legitimate filenames like "good_file.png" — the dot belongs to the name
    // but ".." dots are control tokens.
    let trimmed = raw
        .map(|s| {
            let no_traversal = s.replace("..", "");
            no_traversal
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == '_')
                .collect::<String>()
        })
        .unwrap_or_default();
    if trimmed.is_empty() {
        "blob".to_string()
    } else {
        // limit length
        trimmed.chars().take(120).collect()
    }
}

fn sniff_mime(bytes: &[u8]) -> &'static str {
    // Minimal magic-byte detector; falls back to "application/octet-stream"
    if bytes.len() >= 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
        "image/png"
    } else if bytes.len() >= 3 && &bytes[..3] == b"\xff\xd8\xff" {
        "image/jpeg"
    } else if bytes.len() >= 4 && &bytes[..4] == b"GIF8" {
        "image/gif"
    } else if bytes.len() >= 4 && &bytes[..4] == b"RIFF" {
        "audio/wav"
    } else if bytes.len() >= 4 && &bytes[..4] == b"OggS" {
        "audio/ogg"
    } else if bytes.len() >= 3 && &bytes[..3] == b"ID3" {
        "audio/mpeg"
    } else if bytes.len() >= 4 && &bytes[..4] == b"%PDF" {
        "application/pdf"
    } else {
        "application/octet-stream"
    }
}

fn uploads_dir() -> PathBuf {
    // Mirror the workspace resolution used by main.rs.
    let base = std::env::var("CYBERCLAW_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    base.join(UPLOADS_SUBDIR)
}

async fn upload_file(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, ApiError> {
    let actor = claims.sub.to_string();
    let dir = uploads_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| ApiError::InternalError(format!("create uploads dir: {e}")))?;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::InvalidRequest(format!("malformed multipart: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name != "file" {
            // Skip non-file fields silently.
            continue;
        }
        let original = field.file_name().map(|s| s.to_string());
        let bytes = field
            .bytes()
            .await
            .map_err(|e| ApiError::InvalidRequest(format!("read multipart field: {e}")))?;

        if bytes.len() > MAX_UPLOAD_BYTES {
            if let Some(sink) = state.audit.as_ref() {
                sink.record(AuditEntry::now(
                    actor.clone(),
                    AuditKind::Auth,
                    "upload.create:rejected_too_large".to_string(),
                    None,
                    serde_json::json!({ "size_bytes": bytes.len(), "max": MAX_UPLOAD_BYTES }),
                    AuditResult::Failure {
                        reason: "upload exceeds 25 MB limit".to_string(),
                    },
                ))
                .await;
            }
            return Err(ApiError::InvalidRequest(format!(
                "upload exceeds {MAX_UPLOAD_BYTES} bytes limit"
            )));
        }

        let id = uuid::Uuid::new_v4().to_string();
        let safe_name = sanitize_filename(original.as_deref());
        let stored_filename = format!("{id}-{safe_name}");
        let target = dir.join(&stored_filename);

        // Defensive: ensure resolved path is still under dir (no traversal
        // possible after sanitize but check anyway).
        if !path_is_within(&target, &dir) {
            return Err(ApiError::InvalidRequest(
                "resolved path escaped uploads dir".to_string(),
            ));
        }

        tokio::fs::write(&target, &bytes)
            .await
            .map_err(|e| ApiError::InternalError(format!("persist upload: {e}")))?;

        let mime = sniff_mime(&bytes).to_string();
        let path_str = target.to_string_lossy().to_string();
        let size = bytes.len();

        if let Some(sink) = state.audit.as_ref() {
            sink.record(AuditEntry::now(
                actor.clone(),
                AuditKind::Mutation,
                "upload.create".to_string(),
                Some(format!("upload:{id}")),
                serde_json::json!({
                    "id": id,
                    "size_bytes": size,
                    "mime": mime,
                    "stored_path": path_str,
                    "original_filename": original,
                }),
                AuditResult::Success,
            ))
            .await;
        }

        info!(
            actor = %actor,
            id = %id,
            size = size,
            mime = %mime,
            "upload accepted"
        );

        return Ok(Json(UploadResponse {
            id,
            path: path_str,
            size_bytes: size,
            mime,
            original_filename: original,
        }));
    }

    warn!(actor = %actor, "upload had no 'file' field");
    Err(ApiError::InvalidRequest(
        "multipart body did not contain a 'file' field".to_string(),
    ))
}

fn path_is_within(target: &Path, root: &Path) -> bool {
    let target_canon = target.canonicalize().or_else(|_| {
        // target doesn't exist yet — canonicalize parent + append filename
        target
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .map(|p| p.join(target.file_name().unwrap_or_default()))
            .ok_or(std::io::Error::other("can't resolve parent"))
    });
    let root_canon = root.canonicalize();
    match (target_canon, root_canon) {
        (Ok(t), Ok(r)) => t.starts_with(r),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_filename_strips_traversal() {
        assert_eq!(sanitize_filename(Some("../../etc/passwd")), "etcpasswd");
        assert_eq!(sanitize_filename(Some("good_file.png")), "good_file.png");
        assert_eq!(sanitize_filename(Some("")), "blob");
        assert_eq!(sanitize_filename(None), "blob");
        assert_eq!(sanitize_filename(Some("foo bar.txt")), "foobar.txt");
    }

    #[test]
    fn sniff_mime_known_signatures() {
        assert_eq!(sniff_mime(b"\x89PNG\r\n\x1a\n"), "image/png");
        assert_eq!(sniff_mime(b"\xff\xd8\xff\xe0"), "image/jpeg");
        assert_eq!(sniff_mime(b"GIF8"), "image/gif");
        assert_eq!(sniff_mime(b"RIFF"), "audio/wav");
        assert_eq!(sniff_mime(b"unknown"), "application/octet-stream");
    }
}
