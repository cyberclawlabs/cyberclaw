//! Channels API — IM / webhook channel configuration surface.
//!
//! Sprint 7. Persists enabled channels (Discord, Slack, Telegram, Google
//! Chat, iMessage, Signal, WhatsApp, generic Webhook) to
//! `~/.cyberclaw/channels.toml` using the same on-disk pattern as
//! `users.toml` (Sprint 5).
//!
//! # Routes
//!
//! | Route | Purpose |
//! |---|---|
//! | `GET  /api/v1/channels` | List all channels with enabled flag + config |
//! | `PUT  /api/v1/channels/:id` | Toggle + update one channel's config |
//!
//! # Storage
//!
//! On first boot the store is seeded with the 8 platforms the admin SPA
//! knows about, all disabled. Subsequent mutations are written back to
//! `~/.cyberclaw/channels.toml` as an atomic file rewrite.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    routing::{get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::audit::{AuditEntry, AuditKind, AuditResult};
use crate::error::ApiError;
use crate::middleware::auth::Claims;
use crate::state::AppState;

/// IM channels with real PlatformAdapter implementations in
/// `crates/cyberclaw-connectors/src/im_adapters/`:
///
/// - `lark.rs` — 飞书 / Lark
/// - `telegram.rs` — Telegram
/// - `wechat.rs` — 微信 WeChat
///
/// Plus a generic `webhook` endpoint that any unsupported platform can
/// POST to via `/webhooks/:platform`.
///
/// Other platforms (Discord / Slack / WhatsApp / Signal / Google Chat /
/// iMessage) are on the roadmap but not yet shipped — intentionally NOT
/// listed here so the admin UI reflects what actually works, not what's
/// planned.
pub const DEFAULT_CHANNEL_IDS: &[(&str, &str)] = &[
    ("lark", "Lark (飞书)"),
    ("telegram", "Telegram"),
    ("wechat", "WeChat (微信)"),
    ("webhook", "Generic Webhook"),
];

/// One channel's persisted configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecord {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// Connection status — `"connected" | "disabled" | "error"`.
    #[serde(default)]
    pub status: String,
    /// Free-form per-platform config, serialized as a nested table.
    #[serde(default)]
    pub config: HashMap<String, String>,
}

impl ChannelRecord {
    fn seed(id: &str, name: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            enabled: false,
            status: "disabled".to_string(),
            config: HashMap::new(),
        }
    }
}

/// On-disk shape: `{ channels = [ ChannelRecord, ... ] }`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ChannelsFile {
    #[serde(default)]
    channels: Vec<ChannelRecord>,
}

/// Default location: `$HOME/.cyberclaw/channels.toml`.
pub fn channels_default_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".cyberclaw").join("channels.toml"))
        .unwrap_or_else(|| PathBuf::from(".cyberclaw").join("channels.toml"))
}

/// Load channels from disk, seeding defaults if the file is missing or
/// a channel id from [`DEFAULT_CHANNEL_IDS`] is absent.
pub fn load_channels_from_disk() -> HashMap<String, ChannelRecord> {
    load_channels_from_path(&channels_default_path())
}

/// Load channels from a specific path (test-friendly).
pub fn load_channels_from_path(path: &std::path::Path) -> HashMap<String, ChannelRecord> {
    let mut store: HashMap<String, ChannelRecord> = HashMap::new();

    if path.exists() {
        match std::fs::read_to_string(path) {
            Ok(s) => match toml::from_str::<ChannelsFile>(&s) {
                Ok(file) => {
                    for record in file.channels {
                        store.insert(record.id.clone(), record);
                    }
                }
                Err(err) => {
                    warn!(path = %path.display(), %err, "channels.toml parse failed; seeding defaults");
                }
            },
            Err(err) => {
                warn!(path = %path.display(), %err, "channels.toml read failed; seeding defaults");
            }
        }
    }

    // Seed any default channel that was missing.
    for (id, name) in DEFAULT_CHANNEL_IDS {
        store
            .entry(id.to_string())
            .or_insert_with(|| ChannelRecord::seed(id, name));
    }

    store
}

/// Persist the store to disk. Best-effort — errors are logged but do not
/// poison the in-memory state so the admin can retry.
pub(crate) fn save_channels_to_path(
    path: &std::path::Path,
    store: &HashMap<String, ChannelRecord>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create_dir_all: {e}"))?;
    }
    let mut channels: Vec<ChannelRecord> = store.values().cloned().collect();
    channels.sort_by(|a, b| a.id.cmp(&b.id));
    let file = ChannelsFile { channels };
    let toml_str = toml::to_string_pretty(&file).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(path, toml_str).map_err(|e| format!("write: {e}"))?;
    // chmod 600 — channels.toml may contain webhook_secret values
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// PUT body — all fields optional so the caller can toggle enabled
/// without rewriting the entire config blob.
#[derive(Debug, Deserialize)]
pub struct UpdateChannelRequest {
    pub enabled: Option<bool>,
    pub config: Option<HashMap<String, String>>,
    pub status: Option<String>,
}

/// Build the `/api/v1/channels` router.
pub fn create_channels_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/channels", get(list_channels))
        .route("/api/v1/channels/:id", put(update_channel))
}

/// `GET /api/v1/channels` — list all channels whose platform has either
/// a configured webhook secret OR an explicit `enabled=true` flag in the
/// persisted channels store.
///
/// Sprint 8 Lane 1: the previous implementation returned the full 8-seed
/// list unconditionally. The new shape only surfaces platforms that are
/// actually reachable — a missing platform no longer appears in the list.
///
/// Merge priority (left wins):
/// 1. Persisted [`ChannelRecord`] with `enabled == true`
/// 2. Platform with a configured webhook secret in
///    [`AppState::webhook_secrets`] (derived from
///    `CYBERCLAW_WEBHOOK_SECRET_<PLATFORM>` env vars)
///
/// The returned JSON shape matches the admin SPA's expectation:
/// `{ channels: [{id, name, enabled, webhook_url, secret_configured, config}] }`.
async fn list_channels(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let store = state.channels_store.read().await;
    let secrets = state.webhook_secrets.as_ref();

    let mut out: Vec<serde_json::Value> = Vec::new();
    // Always surface the 8 supported platforms as a catalog so operators can
    // see WHAT'S available, not just what's already configured. Overlay
    // persisted record (from channels.toml) + env webhook secret state.
    for (pid, pname) in DEFAULT_CHANNEL_IDS {
        let record = store.get(*pid);
        let secret_configured = secrets.contains_key(*pid);
        let enabled = record.map(|r| r.enabled).unwrap_or(false) || secret_configured;
        let status = record.map(|r| r.status.clone()).unwrap_or_else(|| {
            if enabled {
                "connected".into()
            } else {
                "disabled".into()
            }
        });
        let config = record
            .map(|r| serde_json::to_value(&r.config).unwrap_or(serde_json::json!({})))
            .unwrap_or_else(|| serde_json::json!({}));
        out.push(serde_json::json!({
            "id": pid,
            "name": pname,
            "enabled": enabled,
            "webhook_url": format!("/webhooks/{}", pid),
            "secret_configured": secret_configured,
            "status": status,
            "config": config,
        }));
    }

    // 2) Add platforms that have a secret but no persisted record yet.
    let known_ids: std::collections::HashSet<String> = out
        .iter()
        .filter_map(|v| v.get("id").and_then(|x| x.as_str()).map(|s| s.to_string()))
        .collect();
    for platform in secrets.keys() {
        if known_ids.contains(platform) {
            continue;
        }
        let name = DEFAULT_CHANNEL_IDS
            .iter()
            .find(|(id, _)| *id == platform)
            .map(|(_, n)| (*n).to_string())
            .unwrap_or_else(|| platform.clone());
        out.push(serde_json::json!({
            "id": platform,
            "name": name,
            "enabled": true,
            "webhook_url": format!("/webhooks/{}", platform),
            "secret_configured": true,
            "status": "connected",
            "config": serde_json::json!({}),
        }));
    }

    // 3) Surface any custom-named channels stored in channels.toml that
    //    aren't in DEFAULT_CHANNEL_IDS and don't have an env webhook secret.
    //    These are user-created via PUT /api/v1/channels/:id with a fresh id
    //    — without this loop the WebUI list would silently drop them.
    let already_listed: std::collections::HashSet<String> = out
        .iter()
        .filter_map(|v| v.get("id").and_then(|x| x.as_str()).map(|s| s.to_string()))
        .collect();
    for (id, record) in store.iter() {
        if already_listed.contains(id) {
            continue;
        }
        out.push(serde_json::json!({
            "id": id,
            "name": record.name,
            "enabled": record.enabled,
            "webhook_url": format!("/webhooks/{}", id),
            "secret_configured": false,
            "status": record.status,
            "config": serde_json::to_value(&record.config).unwrap_or(serde_json::json!({})),
        }));
    }

    out.sort_by(|a, b| {
        a.get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .cmp(b.get("id").and_then(|v| v.as_str()).unwrap_or_default())
    });

    Json(serde_json::json!({
        "channels": out,
        "total": out.len(),
    }))
}

/// `PUT /api/v1/channels/:id` — upsert enabled flag and/or config.
async fn update_channel(
    State(state): State<Arc<AppState>>,
    claims: Option<axum::extract::Extension<Claims>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateChannelRequest>,
) -> Result<Json<ChannelRecord>, ApiError> {
    let actor = claims
        .map(|c| c.0.sub.as_str().to_string())
        .unwrap_or_else(|| "system".to_string());
    let id_trimmed = id.trim();
    if id_trimmed.is_empty() {
        if let Some(sink) = state.audit.as_ref() {
            sink.record(AuditEntry::now(
                actor,
                AuditKind::Config,
                "channel.update".to_string(),
                None,
                serde_json::json!({}),
                AuditResult::Failure {
                    reason: "channel id is required".to_string(),
                },
            ))
            .await;
        }
        return Err(ApiError::InvalidRequest(
            "channel id is required".to_string(),
        ));
    }

    let mut store = state.channels_store.write().await;
    let record = store.entry(id_trimmed.to_string()).or_insert_with(|| {
        // Unknown id — still writable. Name defaults to the id itself.
        let name = DEFAULT_CHANNEL_IDS
            .iter()
            .find(|(seed_id, _)| *seed_id == id_trimmed)
            .map(|(_, name)| *name)
            .unwrap_or(id_trimmed);
        ChannelRecord::seed(id_trimmed, name)
    });

    if let Some(enabled) = req.enabled {
        record.enabled = enabled;
        record.status = if enabled {
            "connected".to_string()
        } else {
            "disabled".to_string()
        };
    }
    if let Some(config) = req.config {
        record.config = config;
    }
    if let Some(status) = req.status {
        record.status = status;
    }

    let updated = record.clone();
    let snapshot = store.clone();
    drop(store);

    if let Err(err) = save_channels_to_path(&channels_default_path(), &snapshot) {
        warn!(channel_id = %id_trimmed, err = %err, "failed to persist channels.toml");
        // Intentionally non-fatal: in-memory state already reflects the
        // mutation, admin can retry. Client sees the new record.
    } else {
        info!(channel_id = %id_trimmed, "channel updated and persisted");
    }

    if let Some(sink) = state.audit.as_ref() {
        sink.record(AuditEntry::now(
            actor,
            AuditKind::Config,
            "channel.update".to_string(),
            Some(format!("channel:{}", id_trimmed)),
            serde_json::json!({ "enabled": updated.enabled }),
            AuditResult::Success,
        ))
        .await;
    }

    Ok(Json(updated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_seeds_defaults_when_file_missing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("channels.toml");
        let store = load_channels_from_path(&path);
        assert_eq!(store.len(), DEFAULT_CHANNEL_IDS.len());
        // Current default set: lark, telegram, wechat, webhook.
        // discord/slack/etc. are intentionally excluded (not yet shipped).
        assert!(store.contains_key("lark"));
        assert!(store.contains_key("telegram"));
        assert!(store.contains_key("wechat"));
        assert!(store.contains_key("webhook"));
        assert!(store.values().all(|r| !r.enabled));
    }

    #[test]
    fn save_and_reload_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("channels.toml");

        let mut store = load_channels_from_path(&path);
        // Use "lark" — a platform present in DEFAULT_CHANNEL_IDS.
        let lark = store.get_mut("lark").unwrap();
        lark.enabled = true;
        lark.status = "connected".to_string();
        lark.config
            .insert("bot_token".to_string(), "test-token".to_string());
        let lark_snapshot = lark.clone();

        save_channels_to_path(&path, &store).expect("save");

        let reloaded = load_channels_from_path(&path);
        let loaded_lark = reloaded.get("lark").unwrap();
        assert!(loaded_lark.enabled);
        assert_eq!(loaded_lark.status, "connected");
        assert_eq!(
            loaded_lark.config.get("bot_token"),
            Some(&"test-token".to_string())
        );
        assert_eq!(loaded_lark.id, lark_snapshot.id);
    }

    #[test]
    fn load_tolerates_corrupt_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("channels.toml");
        std::fs::write(&path, "\n\nnot valid toml [[[[").unwrap();
        let store = load_channels_from_path(&path);
        // Corruption must not crash; defaults are seeded instead.
        assert_eq!(store.len(), DEFAULT_CHANNEL_IDS.len());
    }

    #[tokio::test]
    async fn channels_list_returns_only_configured_platforms() {
        use crate::api::test_helpers::build_test_state;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        // Test state has no webhook secrets and the default channels store
        // (all disabled). The handler now returns the full DEFAULT_CHANNEL_IDS
        // catalog so operators can see what's available even before configuring.
        let state = build_test_state();
        let app = create_channels_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/channels")
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
        let channels = json["channels"].as_array().unwrap();
        // All DEFAULT_CHANNEL_IDS platforms appear in the catalog, all disabled.
        assert_eq!(
            channels.len(),
            DEFAULT_CHANNEL_IDS.len(),
            "catalog should contain all default platforms; got {:?}",
            channels
        );
        assert!(
            channels.iter().all(|c| c["enabled"] == false),
            "all channels should be disabled when no secret configured"
        );
        assert_eq!(json["total"], DEFAULT_CHANNEL_IDS.len());
    }

    #[tokio::test]
    async fn channels_list_surfaces_webhook_configured_platforms() {
        use crate::api::test_helpers::build_test_state;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use std::sync::Arc;
        use tower::ServiceExt;

        let mut state = build_test_state();
        {
            let mutable = Arc::get_mut(&mut state).expect("unique Arc in test");
            let mut secrets = HashMap::new();
            // "lark" IS in DEFAULT_CHANNEL_IDS; configuring its secret marks it
            // secret_configured=true and enabled=true in the catalog entry.
            secrets.insert("lark".to_string(), "hmac-secret".to_string());
            mutable.webhook_secrets = Arc::new(secrets);
        }

        let app = create_channels_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/channels")
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
        let channels = json["channels"].as_array().unwrap();
        // Catalog always contains all DEFAULT_CHANNEL_IDS platforms.
        assert_eq!(channels.len(), DEFAULT_CHANNEL_IDS.len());
        // Find lark in the sorted result and verify it is marked configured.
        let lark = channels
            .iter()
            .find(|c| c["id"] == "lark")
            .expect("lark must appear in catalog");
        assert_eq!(lark["secret_configured"], true);
        assert_eq!(lark["enabled"], true);
        assert_eq!(lark["webhook_url"], "/webhooks/lark");
        // The remaining platforms must not be marked secret_configured.
        let others_secret: Vec<_> = channels
            .iter()
            .filter(|c| c["id"] != "lark" && c["secret_configured"] == true)
            .collect();
        assert!(
            others_secret.is_empty(),
            "only lark should be secret_configured; others: {:?}",
            others_secret
        );
    }
}
