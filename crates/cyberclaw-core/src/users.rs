//! Minimal operator identity persistence (Sprint 5).
//!
//! This module provides a ruthlessly small user identity layer:
//!
//! - [`OperatorRecord`]: one row per operator account (display name + audit
//!   timestamps) keyed by [`UserId`].
//! - [`UsersConfig`]: an in-memory collection of records with TOML persistence
//!   to `~/.cyberclaw/users.toml`.
//!
//! There is intentionally no database table, no password storage, no RBAC,
//! and no registration flow. The wizard emits an [`OperatorRecord`] when it
//! finishes, and the record is the sole source of truth for "which operators
//! have ever logged in on this installation".
//!
//! # Format
//!
//! ```toml
//! [[users]]
//! user_id = "c1..."
//! display_name = "Alice"
//! created_at = "2026-04-18T12:00:00Z"
//! last_login = "2026-04-18T12:30:00Z"
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::UserId;

/// Errors that can occur loading or saving a [`UsersConfig`].
#[derive(Debug, Error)]
pub enum UsersConfigError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML deserialization error: {0}")]
    Deserialize(#[from] toml::de::Error),

    #[error("TOML serialization error: {0}")]
    Serialize(#[from] toml::ser::Error),
}

/// One operator account.
///
/// Timestamps are stored as ISO 8601 strings (e.g. `2026-04-18T12:00:00Z`).
/// No password, no permissions, no tenant binding — that lives elsewhere or
/// not at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorRecord {
    pub user_id: UserId,
    pub display_name: String,
    pub created_at: String,
    pub last_login: Option<String>,
    /// Role — authoritative source is this field, persisted server-side.
    /// MUST NOT be overridden by client-side state. Sensitive endpoints
    /// (reviews approve/reject, settings/env update, seed-demo) check
    /// this against the caller's JWT-asserted user_id.
    /// Default "admin" for the very first operator registered on the
    /// system; subsequent operators default to "viewer" unless an admin
    /// explicitly promotes them.
    #[serde(default = "default_role")]
    pub role: String,
    /// Timestamp (ISO 8601) when the operator completed the admin console
    /// onboarding wizard. `None` means the operator has never finished it —
    /// the admin SPA shows the onboarding dialog on next login.
    ///
    /// Separate from `last_login` so a returning operator on a new device
    /// does not get re-onboarded.
    #[serde(default)]
    pub onboarded_at: Option<String>,
    /// Opt-in flag: when `true`, chat requests whose intent is classified as
    /// `CreateSkill` AND whose user message contains a parseable skill spec
    /// are automatically routed to the `/api/v1/skills/create` logic instead
    /// of going to the LLM. Can be overridden globally by
    /// `CYBERCLAW_CHAT_AUTO_ROUTE=off|advisor|auto`.
    #[serde(default)]
    pub intent_auto_route: bool,
}

fn default_role() -> String {
    "viewer".to_string()
}

/// Persistent collection of [`OperatorRecord`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsersConfig {
    #[serde(default)]
    pub users: Vec<OperatorRecord>,
}

impl UsersConfig {
    /// Load from `path`. Returns the default (empty) config if the file does
    /// not exist; all other errors propagate.
    pub fn load_from_path(path: &Path) -> Result<Self, UsersConfigError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = std::fs::read_to_string(path)?;
        let cfg: UsersConfig = toml::from_str(&data)?;
        Ok(cfg)
    }

    /// Write to `path`. Creates the parent directory if missing.
    pub fn save_to_path(&self, path: &Path) -> Result<(), UsersConfigError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let toml_str = toml::to_string_pretty(self)?;
        std::fs::write(path, toml_str)?;
        // chmod 600 — users.toml contains operator role information
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    /// Find a record by `user_id`.
    pub fn find(&self, user_id: &UserId) -> Option<&OperatorRecord> {
        self.users.iter().find(|r| &r.user_id == user_id)
    }

    /// Insert a new record or replace the existing one with the same
    /// `user_id`.
    pub fn upsert(&mut self, record: OperatorRecord) {
        if let Some(existing) = self.users.iter_mut().find(|r| r.user_id == record.user_id) {
            *existing = record;
        } else {
            self.users.push(record);
        }
    }

    /// Default on-disk location: `$HOME/.cyberclaw/users.toml`. Falls back to
    /// `.cyberclaw/users.toml` in the current directory if `$HOME` is unset.
    pub fn default_path() -> PathBuf {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".cyberclaw").join("users.toml"))
            .unwrap_or_else(|| PathBuf::from(".cyberclaw").join("users.toml"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // HOME env mutation needs a lock across tests in this module.
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    fn sample_record(display: &str) -> OperatorRecord {
        OperatorRecord {
            user_id: UserId::new(),
            display_name: display.to_string(),
            created_at: "2026-04-18T12:00:00Z".to_string(),
            last_login: None,
            role: "admin".to_string(),
            onboarded_at: None,
            intent_auto_route: false,
        }
    }

    #[test]
    fn users_config_default_is_empty() {
        let cfg = UsersConfig::default();
        assert!(cfg.users.is_empty());
    }

    #[test]
    fn users_config_save_load_roundtrip() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("users.toml");

        let mut cfg = UsersConfig::default();
        let rec = sample_record("Alice");
        let rec_id = rec.user_id.clone();
        cfg.upsert(rec);

        cfg.save_to_path(&path).expect("save ok");
        assert!(path.exists(), "file must exist after save");

        let loaded = UsersConfig::load_from_path(&path).expect("load ok");
        assert_eq!(loaded.users.len(), 1);
        assert_eq!(loaded.users[0].user_id, rec_id);
        assert_eq!(loaded.users[0].display_name, "Alice");
    }

    #[test]
    fn users_config_find_by_id() {
        let mut cfg = UsersConfig::default();
        let a = sample_record("Alice");
        let b = sample_record("Bob");
        let b_id = b.user_id.clone();
        cfg.upsert(a);
        cfg.upsert(b);

        let found = cfg.find(&b_id).expect("found");
        assert_eq!(found.display_name, "Bob");

        let missing = UserId::new();
        assert!(cfg.find(&missing).is_none());
    }

    #[test]
    fn users_config_upsert_overwrites_same_id() {
        let mut cfg = UsersConfig::default();
        let rec = sample_record("Alice");
        let id = rec.user_id.clone();
        cfg.upsert(rec);
        assert_eq!(cfg.users.len(), 1);
        assert_eq!(cfg.users[0].display_name, "Alice");

        let updated = OperatorRecord {
            role: "admin".to_string(),
            user_id: id.clone(),
            display_name: "Alice Renamed".into(),
            created_at: "2026-04-18T12:00:00Z".into(),
            last_login: Some("2026-04-18T13:00:00Z".into()),
            onboarded_at: None,
            intent_auto_route: false,
        };
        cfg.upsert(updated);
        assert_eq!(cfg.users.len(), 1, "upsert with same id must not duplicate");
        assert_eq!(cfg.users[0].display_name, "Alice Renamed");
        assert_eq!(
            cfg.users[0].last_login.as_deref(),
            Some("2026-04-18T13:00:00Z")
        );
    }

    #[test]
    fn users_config_default_path_under_home() {
        let _guard = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().expect("tempdir");
        let original = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }
        let path = UsersConfig::default_path();
        // Restore HOME regardless of assertion outcome below.
        unsafe {
            match original {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        assert_eq!(path, tmp.path().join(".cyberclaw").join("users.toml"));
    }

    #[test]
    fn users_config_load_missing_file_returns_default() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("does-not-exist.toml");
        let loaded = UsersConfig::load_from_path(&path).expect("load ok");
        assert!(loaded.users.is_empty());
    }
}
