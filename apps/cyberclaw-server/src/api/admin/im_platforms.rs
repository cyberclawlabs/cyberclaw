//! Admin IM platform endpoints.
//!
//! | Method | Path                                      | Purpose                                      |
//! |--------|-------------------------------------------|----------------------------------------------|
//! | GET    | `/api/v1/admin/im-platforms`              | List known IM platforms (configured + not)   |
//! | PUT    | `/api/v1/admin/im-platforms/:name/config` | Persist a platform's config to channels.toml |
//! | POST   | `/api/v1/admin/im-platforms/:name/test`   | Send a test message via the platform         |
//!
//! IM platform connectors live in `cyberclaw-connectors`:
//! - `ImChannelConnector` (id `im-channel`) hosts pluggable
//!   `ImPlatformAdapter` instances (lark / telegram / wechat / discord ...)
//! - `SlackConnector` (id `slack-im`) is a separate first-class connector
//!
//! Config is persisted to the existing `channels.toml` file (see
//! `crate::api::channels`) so the runtime stays self-contained: the admin
//! console reuses the same store the rest of the channel surface already
//! reads from. Secret-bearing fields (signing_key, bot_token_ref) are
//! redacted from list responses.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    routing::{get, post, put},
    Json, Router,
};
use cyberclaw_connectors::im_adapters::{
    DiscordAdapter, LineAdapter, LineConfig, SlackAdapter, TelegramAdapter, TelegramConfig,
};
use cyberclaw_connectors::im_channel::ImPlatformAdapter;
use serde::{Deserialize, Serialize};

use crate::api::channels::{channels_default_path, save_channels_to_path, ChannelRecord};
use crate::error::ApiError;
use crate::state::AppState;

// Substring patterns that mark a config field as secret-bearing. Any key
// whose lowercased name CONTAINS one of these substrings is redacted from
// list responses AND must arrive as a SecretRef on PUT.
//
// H-2: substring matching (not exact match) closes the "auth_header" /
// "credential" / "private_key" / "passphrase" loophole the original
// allowlist had. New secret-bearing keys added downstream automatically
// inherit the redaction.
const SECRET_PATTERNS: &[&str] = &[
    "token",
    "secret",
    "key",
    "password",
    "passwd",
    "passphrase",
    "credential",
    "auth",
    "private",
];

fn is_secret_field(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SECRET_PATTERNS.iter().any(|p| lower.contains(p))
}

// ---------------------------------------------------------------------------
// GET /api/v1/admin/im-platforms
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ImPlatformRow {
    pub name: String,
    pub kind: String,
    pub configured: bool,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Redacted snapshot of the persisted config (secret values blanked).
    pub config: HashMap<String, String>,
}

/// Hard-coded list of known IM platform kinds. Mirrors
/// `crates/cyberclaw-connectors/src/im_adapters/`.
const KNOWN_KINDS: &[&str] = &[
    "discord", "slack", "lark", "line", "telegram", "wechat", "webhook",
];

fn redact_config(config: &HashMap<String, String>) -> HashMap<String, String> {
    config
        .iter()
        .map(|(k, v)| {
            if is_secret_field(k) {
                (k.clone(), "<redacted>".to_string())
            } else {
                (k.clone(), v.clone())
            }
        })
        .collect()
}

fn record_required_secret_field(kind: &str) -> Option<&'static str> {
    match kind {
        "discord" => Some("bot_token"),
        "slack" => Some("bot_token"),
        "lark" => Some("app_id"),
        "line" => Some("channel_access_token"),
        "telegram" => Some("bot_token"),
        "wechat" => Some("app_id"),
        // Generic webhook only needs an external URL → considered configured
        // when we have any non-empty config value.
        "webhook" => None,
        _ => None,
    }
}

fn is_configured(record: Option<&ChannelRecord>, kind: &str) -> bool {
    let Some(rec) = record else {
        return false;
    };
    if !rec.enabled {
        return false;
    }
    match record_required_secret_field(kind) {
        Some(field) => rec.config.get(field).is_some_and(|v| !v.is_empty()),
        None => !rec.config.is_empty(),
    }
}

pub async fn list_platforms(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ImPlatformRow>>, ApiError> {
    let store = state.channels_store.read().await;

    // Catalog every known kind so the admin UI sees the full surface, then
    // overlay any persisted record from channels.toml.
    let mut rows = Vec::new();
    for kind in KNOWN_KINDS {
        let rec = store.get(*kind);
        let configured = is_configured(rec, kind);
        let status = match rec {
            Some(r) if !r.status.is_empty() => r.status.clone(),
            _ if configured => "ok".to_string(),
            _ => "unconfigured".to_string(),
        };
        let config = rec.map(|r| redact_config(&r.config)).unwrap_or_default();
        rows.push(ImPlatformRow {
            name: kind.to_string(),
            kind: kind.to_string(),
            configured,
            status,
            last_error: None,
            config,
        });
    }

    // Surface any other persisted records that aren't in the known catalog.
    for (id, rec) in store.iter() {
        if KNOWN_KINDS.contains(&id.as_str()) {
            continue;
        }
        rows.push(ImPlatformRow {
            name: id.clone(),
            kind: id.clone(),
            configured: rec.enabled && !rec.config.is_empty(),
            status: if rec.status.is_empty() {
                if rec.enabled {
                    "ok".to_string()
                } else {
                    "unconfigured".to_string()
                }
            } else {
                rec.status.clone()
            },
            last_error: None,
            config: redact_config(&rec.config),
        });
    }

    Ok(Json(rows))
}

// ---------------------------------------------------------------------------
// PUT /api/v1/admin/im-platforms/:name/config
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct UpdatePlatformConfigRequest {
    /// Optional kind override. Defaults to the URL `:name` segment.
    #[serde(default)]
    pub kind: Option<String>,
    /// Free-form per-platform config. Secret-bearing keys are accepted as
    /// SecretRef strings (e.g. `"env:CYBERCLAW_DISCORD_BOT_TOKEN"`).
    #[serde(flatten)]
    pub fields: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct UpdatePlatformConfigResponse {
    pub ok: bool,
    /// Whether the running connector picks up the new config without a
    /// process restart. Currently `false` for all kinds — channels.toml is
    /// loaded once at startup and the in-process adapter map is fixed at
    /// boot time. Operators must restart the server to apply.
    pub hot_reload: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn extract_string_fields(fields: &HashMap<String, serde_json::Value>) -> HashMap<String, String> {
    fields
        .iter()
        .filter_map(|(k, v)| match v {
            serde_json::Value::String(s) => Some((k.clone(), s.clone())),
            serde_json::Value::Number(n) => Some((k.clone(), n.to_string())),
            serde_json::Value::Bool(b) => Some((k.clone(), b.to_string())),
            serde_json::Value::Null => None,
            other => serde_json::to_string(other).ok().map(|s| (k.clone(), s)),
        })
        .collect()
}

fn validate_secret_ref(key: &str, value: &str) -> Result<(), ApiError> {
    if !is_secret_field(key) {
        return Ok(());
    }
    // Accept SecretRef shapes: env:NAME, file:/path, vault://..., secret://...
    if value.is_empty() {
        return Ok(()); // empty clears the slot
    }
    // H-3: do NOT accept "<redacted>" here. It's a presentation-layer
    // placeholder, not a SecretRef. The caller short-circuits "<redacted>"
    // BEFORE this validator runs (treated as "leave as-is"); reaching
    // this branch with the placeholder means a logic error and must
    // never silently land in storage.
    let is_secret_ref = value.starts_with("env:")
        || value.starts_with("file:")
        || value.starts_with("vault:")
        || value.starts_with("secret:");
    if !is_secret_ref {
        return Err(ApiError::InvalidInput(format!(
            "{} must be a SecretRef (env:NAME / file:/path / vault://...) and never plaintext",
            key
        )));
    }
    Ok(())
}

pub async fn update_platform_config(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(req): Json<UpdatePlatformConfigRequest>,
) -> Result<Json<UpdatePlatformConfigResponse>, ApiError> {
    if name.trim().is_empty() {
        return Err(ApiError::InvalidInput("platform name required".to_string()));
    }
    let kind = req.kind.clone().unwrap_or_else(|| name.clone());
    let fields = extract_string_fields(&req.fields);

    // H-3: short-circuit `<redacted>` BEFORE running secret-ref validation
    // so the placeholder never traverses the validator (and never lands
    // in storage). Anything else still has to clear validate_secret_ref.
    let mut applied: Vec<(String, String)> = Vec::with_capacity(fields.len());
    for (k, v) in fields {
        if v == "<redacted>" {
            // Caller round-tripped a redacted GET — leave the slot as-is.
            continue;
        }
        validate_secret_ref(&k, &v)?;
        applied.push((k, v));
    }

    let mut store = state.channels_store.write().await;
    let entry = store.entry(name.clone()).or_insert_with(|| ChannelRecord {
        id: name.clone(),
        name: kind.clone(),
        enabled: false,
        status: "unconfigured".to_string(),
        config: HashMap::new(),
    });

    for (k, v) in applied {
        entry.config.insert(k, v);
    }
    entry.enabled = is_configured(Some(entry), kind.as_str()) || entry.enabled;
    entry.status = if entry.enabled {
        "ok".to_string()
    } else {
        "unconfigured".to_string()
    };

    // H-1: persist to channels.toml so the change survives restart. Mirror
    // the pattern in `crate::api::channels::update_channel`. Persist
    // best-effort; surface a `note` on failure but keep the in-memory
    // mutation so the operator can retry.
    let snapshot = store.clone();
    drop(store);
    let mut persist_note: Option<String> = Some(
        "config persisted to channels.toml; restart the server to attach a new adapter to the running ImChannelConnector"
            .to_string(),
    );
    if let Err(err) = save_channels_to_path(&channels_default_path(), &snapshot) {
        tracing::warn!(platform = %name, err = %err, "failed to persist im-platform config");
        persist_note = Some(format!(
            "in-memory mutation applied but channels.toml persist failed: {} \
             — retry to capture the change on disk",
            err
        ));
    }

    Ok(Json(UpdatePlatformConfigResponse {
        ok: true,
        hot_reload: false,
        note: persist_note,
    }))
}

// ---------------------------------------------------------------------------
// POST /api/v1/admin/im-platforms/:name/test
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TestPlatformRequest {
    #[serde(default)]
    pub channel_id: Option<String>,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct TestPlatformResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 构建对应 platform 的 ImPlatformAdapter；如果 name 不支持或缺 token 返回 Err 文本。
fn build_test_adapter(
    name: &str,
    config: &HashMap<String, String>,
) -> Result<Box<dyn ImPlatformAdapter>, String> {
    let token_field = |key: &str| -> Result<String, String> {
        config
            .get(key)
            .filter(|v| !v.is_empty())
            .cloned()
            .ok_or_else(|| format!("missing required config field '{}' for {}", key, name))
    };
    match name {
        "slack" => {
            let token = token_field("bot_token")?;
            let mut a = SlackAdapter::new(token);
            if let Some(sec) = config.get("signing_secret").filter(|v| !v.is_empty()) {
                a = a.with_signing_secret(sec.clone());
            }
            Ok(Box::new(a))
        }
        "telegram" => {
            let token = token_field("bot_token")?;
            let cfg = TelegramConfig::new(token);
            let adapter = TelegramAdapter::new(cfg)
                .map_err(|e| format!("telegram adapter init failed: {}", e))?;
            Ok(Box::new(adapter))
        }
        "discord" => {
            let token = token_field("bot_token")?;
            let mut a = DiscordAdapter::new(token);
            if let Some(pk) = config.get("public_key").filter(|v| !v.is_empty()) {
                a = a.with_public_key(pk.clone());
            }
            Ok(Box::new(a))
        }
        "line" => {
            let channel_access_token = token_field("channel_access_token")?;
            let channel_secret = token_field("channel_secret")?;
            Ok(Box::new(LineAdapter::new(LineConfig {
                channel_access_token,
                channel_secret,
            })))
        }
        // lark / wechat 暂用 config 结构体，admin REST 不直接构造（v1.2.x）
        other => Err(format!(
            "platform '{}' not wired in admin REST test send (v1.2.x backlog)",
            other
        )),
    }
}

pub async fn test_platform(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(req): Json<TestPlatformRequest>,
) -> Result<Json<TestPlatformResponse>, ApiError> {
    if req.text.trim().is_empty() {
        return Err(ApiError::InvalidInput("text must not be empty".to_string()));
    }
    let channel_id = req.channel_id.unwrap_or_default();
    if channel_id.trim().is_empty() {
        return Ok(Json(TestPlatformResponse {
            ok: false,
            message_id: None,
            error: Some("channel_id is required to dispatch a test message".to_string()),
        }));
    }

    // 从 channels_store 取出该 platform 的持久化配置
    let config: HashMap<String, String> = {
        let store = state.channels_store.read().await;
        match store.get(&name) {
            Some(rec) if rec.enabled => rec.config.clone(),
            Some(_) => {
                return Ok(Json(TestPlatformResponse {
                    ok: false,
                    message_id: None,
                    error: Some(format!("platform '{}' is configured but disabled", name)),
                }));
            }
            None => {
                return Ok(Json(TestPlatformResponse {
                    ok: false,
                    message_id: None,
                    error: Some(format!(
                        "platform '{}' has no persisted config — PUT /api/v1/admin/im-platforms/{}/config first",
                        name, name
                    )),
                }));
            }
        }
    };

    // 构建 adapter（slack/telegram/discord 真接线，lark/wechat 仍走 backlog）
    let adapter = match build_test_adapter(&name, &config) {
        Ok(a) => a,
        Err(msg) => {
            return Ok(Json(TestPlatformResponse {
                ok: false,
                message_id: None,
                error: Some(msg),
            }));
        }
    };

    // 实际派送
    match adapter.send_text(&channel_id, &req.text).await {
        Ok(_) => Ok(Json(TestPlatformResponse {
            ok: true,
            message_id: None,
            error: None,
        })),
        Err(e) => Ok(Json(TestPlatformResponse {
            ok: false,
            message_id: None,
            error: Some(format!("send failed: {:#}", e)),
        })),
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn create_admin_im_platforms_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/admin/im-platforms", get(list_platforms))
        .route(
            "/api/v1/admin/im-platforms/:name/config",
            put(update_platform_config),
        )
        .route("/api/v1/admin/im-platforms/:name/test", post(test_platform))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[serial_test::serial]
    async fn list_returns_known_kinds_unconfigured_by_default() {
        // Redirect $HOME to an empty temp dir so load_channels_from_disk()
        // never reads a developer's real ~/.cyberclaw/channels.toml and so
        // concurrent tests that mutate $HOME cannot race with us.
        let tmp = tempfile::TempDir::new().unwrap();
        let _home_guard = crate::api::test_helpers::RestoreHome::set(tmp.path());

        let state = crate::api::test_helpers::build_test_state();
        let resp = list_platforms(State(state)).await.unwrap().0;
        let mut kinds: Vec<String> = resp.iter().map(|r| r.kind.clone()).collect();
        kinds.sort();
        // All KNOWN_KINDS show up at minimum.
        for k in KNOWN_KINDS {
            assert!(kinds.contains(&k.to_string()), "missing kind {}", k);
        }
        // Catalog rows start unconfigured.
        for r in &resp {
            if KNOWN_KINDS.contains(&r.kind.as_str()) {
                assert!(!r.configured, "kind {} should default unconfigured", r.kind);
            }
        }
    }

    #[tokio::test]
    async fn put_rejects_plaintext_token() {
        let state = crate::api::test_helpers::build_test_state();
        let mut fields = HashMap::new();
        fields.insert(
            "bot_token".to_string(),
            serde_json::Value::String("xoxb-PLAINTEXT".to_string()),
        );
        let req = UpdatePlatformConfigRequest {
            kind: Some("slack".to_string()),
            fields,
        };
        let res = update_platform_config(State(state), Path("slack".to_string()), Json(req)).await;
        assert!(matches!(res, Err(ApiError::InvalidInput(_))));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn put_persists_secret_ref() {
        // H-1: PUT now writes channels.toml under $HOME. Redirect HOME so
        // the test does not pollute the operator's real ~/.cyberclaw.
        let tmp = tempfile::TempDir::new().unwrap();
        let original = std::env::var_os("HOME");
        // SAFETY: integration tests in this module are sequential; this
        // module owns the env mutation for the duration of the test.
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }
        struct HomeGuard {
            original: Option<std::ffi::OsString>,
        }
        impl Drop for HomeGuard {
            fn drop(&mut self) {
                unsafe {
                    match self.original.take() {
                        Some(v) => std::env::set_var("HOME", v),
                        None => std::env::remove_var("HOME"),
                    }
                }
            }
        }
        let _guard = HomeGuard { original };

        let state = crate::api::test_helpers::build_test_state();
        let mut fields = HashMap::new();
        fields.insert(
            "bot_token".to_string(),
            serde_json::Value::String("env:DISCORD_BOT_TOKEN".to_string()),
        );
        fields.insert(
            "guild_id".to_string(),
            serde_json::Value::String("123".to_string()),
        );
        let req = UpdatePlatformConfigRequest {
            kind: Some("discord".to_string()),
            fields,
        };
        let resp =
            update_platform_config(State(state.clone()), Path("discord".to_string()), Json(req))
                .await
                .unwrap();
        assert!(resp.0.ok);
        // List response must redact secret values.
        let list = list_platforms(State(state)).await.unwrap().0;
        let row = list.iter().find(|r| r.name == "discord").unwrap();
        assert_eq!(row.config.get("bot_token").unwrap(), "<redacted>");
        assert_eq!(row.config.get("guild_id").unwrap(), "123");

        // H-1 regression: the new record must have hit channels.toml.
        let toml_path = tmp.path().join(".cyberclaw").join("channels.toml");
        assert!(
            toml_path.exists(),
            "PUT must persist to channels.toml at {}",
            toml_path.display()
        );
        let body = std::fs::read_to_string(&toml_path).unwrap();
        assert!(body.contains("env:DISCORD_BOT_TOKEN"));
        assert!(body.contains("guild_id"));
    }

    /// H-2 regression: substring-pattern secret detection must catch
    /// non-allowlisted secret-bearing keys (auth_header, passphrase,
    /// private_key, credential).
    #[test]
    fn h2_secret_pattern_covers_renamed_fields() {
        for key in [
            "auth_header",
            "passphrase",
            "private_key",
            "credential",
            "Webhook_Password",
            "API_TOKEN",
        ] {
            assert!(is_secret_field(key), "{} must be flagged as secret", key);
        }
        for key in ["app_id", "channel_id", "kind", "enabled", "guild_id"] {
            assert!(
                !is_secret_field(key),
                "{} must NOT be flagged as secret",
                key
            );
        }
    }

    /// H-3 regression: the literal `<redacted>` string must NOT pass the
    /// SecretRef validator (it's only honoured as a higher-level "leave
    /// as-is" signal in update_platform_config).
    #[test]
    fn h3_redacted_placeholder_is_not_a_secret_ref() {
        let res = validate_secret_ref("bot_token", "<redacted>");
        assert!(matches!(res, Err(ApiError::InvalidInput(_))));
    }

    #[tokio::test]
    async fn test_send_without_connector_returns_ok_false() {
        let state = crate::api::test_helpers::build_test_state();
        let resp = test_platform(
            State(state),
            Path("discord".to_string()),
            Json(TestPlatformRequest {
                channel_id: Some("C1".to_string()),
                text: "hi".to_string(),
            }),
        )
        .await
        .unwrap();
        assert!(!resp.0.ok);
        assert!(resp.0.error.is_some());
    }
}
