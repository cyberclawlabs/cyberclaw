//! User profile store — thin user-level preference layer on top of Agent.
//!
//! # Overview
//!
//! * [`Profile`] — persisted profile definition (SOUL string + LLM tuning).
//! * [`ProfileStore`] — in-memory `Mutex<HashMap>` + file backup at
//!   `~/.cyberclaw/profiles.toml` (chmod 600 on every write).
//! * Methods: `list()`, `get(id)`, `create(profile)`, `update(id, f)`,
//!   `delete(id)`.
//!
//! Profile is NOT a platform object (Agent/Skill/Connector/Capability/Plugin).
//! It is a user-level preference stored locally and scoped by `owner_user_id`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

/// A named user-level personality preference ("SOUL").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    /// Display name chosen by the user, e.g. "Coding mode".
    pub name: String,
    /// The SOUL string injected as a system prompt when this profile is active.
    pub system_prompt: String,
    /// Which Agent to use when this Profile is active (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_agent_id: Option<String>,
    /// Which model to use, e.g. "claude-opus-4-5" (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    /// LLM temperature tuning (optional, 0.0–2.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Max tokens tuning (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// User id that owns this profile.
    pub owner_user_id: String,
}

// ---------------------------------------------------------------------------
// On-disk shape
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
struct ProfileFile {
    #[serde(default)]
    profiles: Vec<Profile>,
}

// ---------------------------------------------------------------------------
// ProfileStore
// ---------------------------------------------------------------------------

/// Thread-safe in-memory store backed by a TOML file on disk.
///
/// All mutating methods write-through to disk immediately.
pub struct ProfileStore {
    inner: Mutex<HashMap<String, Profile>>,
    path: PathBuf,
}

impl ProfileStore {
    /// Create a new store, loading existing profiles from `path` if it exists.
    pub fn load(path: PathBuf) -> Arc<Self> {
        let map = load_from_path(&path);
        Arc::new(Self {
            inner: Mutex::new(map),
            path,
        })
    }

    /// Default path: `$HOME/.cyberclaw/profiles.toml`.
    pub fn default_path() -> PathBuf {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".cyberclaw").join("profiles.toml"))
            .unwrap_or_else(|| PathBuf::from(".cyberclaw").join("profiles.toml"))
    }

    /// Load from the default path.
    pub fn load_default() -> Arc<Self> {
        Self::load(Self::default_path())
    }

    /// Return all profiles sorted by name, optionally filtered by owner.
    pub fn list(&self, owner_filter: Option<&str>) -> Vec<Profile> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut profiles: Vec<Profile> = guard
            .values()
            .filter(|p| owner_filter.map(|o| p.owner_user_id == o).unwrap_or(true))
            .cloned()
            .collect();
        profiles.sort_by(|a, b| a.name.cmp(&b.name));
        profiles
    }

    /// Return a single profile by id, or `None`.
    pub fn get(&self, id: &str) -> Option<Profile> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.get(id).cloned()
    }

    /// Insert a new profile. Returns the inserted profile.
    pub fn create(&self, profile: Profile) -> Profile {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(profile.id.clone(), profile.clone());
        self.persist(&guard);
        profile
    }

    /// Update fields of an existing profile (by id). Returns the updated
    /// profile or `None` if the id is not found.
    pub fn update<F>(&self, id: &str, f: F) -> Option<Profile>
    where
        F: FnOnce(&mut Profile),
    {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let profile = guard.get_mut(id)?;
        f(profile);
        profile.updated_at = Utc::now();
        let profile = profile.clone();
        self.persist(&guard);
        Some(profile)
    }

    /// Remove a profile by id. Returns `true` if it existed.
    pub fn delete(&self, id: &str) -> bool {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let existed = guard.remove(id).is_some();
        if existed {
            self.persist(&guard);
        }
        existed
    }

    // ------------------------------------------------------------------
    // Persistence helpers
    // ------------------------------------------------------------------

    fn persist(&self, guard: &HashMap<String, Profile>) {
        if let Err(e) = save_to_path(&self.path, guard) {
            warn!(path = %self.path.display(), error = %e, "profiles.toml write failed");
        }
    }
}

// ---------------------------------------------------------------------------
// Disk I/O helpers
// ---------------------------------------------------------------------------

fn load_from_path(path: &std::path::Path) -> HashMap<String, Profile> {
    if !path.exists() {
        return HashMap::new();
    }
    match std::fs::read_to_string(path) {
        Ok(s) => match toml::from_str::<ProfileFile>(&s) {
            Ok(file) => file
                .profiles
                .into_iter()
                .map(|p| (p.id.clone(), p))
                .collect(),
            Err(e) => {
                warn!(path = %path.display(), error = %e, "profiles.toml parse failed; starting empty");
                HashMap::new()
            }
        },
        Err(e) => {
            warn!(path = %path.display(), error = %e, "profiles.toml read failed; starting empty");
            HashMap::new()
        }
    }
}

fn save_to_path(path: &std::path::Path, map: &HashMap<String, Profile>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create_dir_all: {e}"))?;
    }
    let mut profiles: Vec<Profile> = map.values().cloned().collect();
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    let file = ProfileFile { profiles };
    let toml_str = toml::to_string_pretty(&file).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(path, &toml_str).map_err(|e| format!("write: {e}"))?;
    // chmod 600 — only owner can read/write
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Factory helper
// ---------------------------------------------------------------------------

/// Build a new [`Profile`] from user-supplied fields. Generates id and
/// timestamps automatically.
pub fn build_new_profile(
    name: String,
    system_prompt: String,
    default_agent_id: Option<String>,
    default_model: Option<String>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    owner_user_id: String,
) -> Profile {
    let now = Utc::now();
    Profile {
        id: Uuid::new_v4().to_string(),
        name,
        system_prompt,
        default_agent_id,
        default_model,
        temperature,
        max_tokens,
        created_at: now,
        updated_at: now,
        owner_user_id,
    }
}
