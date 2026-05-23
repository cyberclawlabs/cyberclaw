//! User-installed package persistence.
//!
//! Stores the list of packages added via `cyberclaw-cli package install`
//! (or `POST /api/v2/packages`) at `~/.cyberclaw/installed-packages.json`
//! so they survive a server restart.
//!
//! # Lifecycle
//!
//! 1. Server boot: [`InstalledPackageStore::load_default`] reads the JSON
//!    file (or returns an empty store if it's missing) and hands the
//!    records to [`super::api::packages::bootstrap_user_packages`] so the
//!    in-memory [`cyberclaw_control_plane::registry::InMemoryRegistry`] is
//!    populated **before** ecosystem auto-scan happens (so user packages
//!    are visible in `list_*` paths).
//! 2. `POST /api/v2/packages`: handler loads the manifest, upserts into the
//!    registry, and calls [`InstalledPackageStore::upsert`] to persist.
//! 3. `DELETE /api/v2/packages/...`: handler removes the registry entry
//!    and calls [`InstalledPackageStore::remove`].
//!
//! The persisted shape is intentionally minimal — just enough to re-load
//! the manifest from disk on boot. We do **not** persist the full
//! `PackageManifest` (it can drift if the on-disk source changes); we
//! re-read the manifest from `source` on boot.

use std::path::PathBuf;
use std::sync::Mutex;

use cyberclaw_core::manifests::PackageKind;
use serde::{Deserialize, Serialize};

/// One persisted user-installed package row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPackageRecord {
    pub kind: PackageKind,
    pub id: String,
    /// Local filesystem path that holds the package's `manifest.yaml`.
    pub source_path: String,
    /// Version observed at the time of install. Re-validated on boot.
    pub version: String,
}

/// Top-level JSON shape.
#[derive(Debug, Default, Serialize, Deserialize)]
struct InstalledPackagesFile {
    #[serde(default)]
    packages: Vec<InstalledPackageRecord>,
}

/// In-process snapshot + file backing for `~/.cyberclaw/installed-packages.json`.
///
/// Mutations write through to disk on every call. Reads are lock-only.
pub struct InstalledPackageStore {
    inner: Mutex<Vec<InstalledPackageRecord>>,
    path: PathBuf,
}

impl InstalledPackageStore {
    /// Default path: `$HOME/.cyberclaw/installed-packages.json`.
    pub fn default_path() -> PathBuf {
        std::env::var_os("HOME")
            .map(|h| {
                PathBuf::from(h)
                    .join(".cyberclaw")
                    .join("installed-packages.json")
            })
            .unwrap_or_else(|| {
                PathBuf::from(".cyberclaw").join("installed-packages.json")
            })
    }

    /// Load from `path`. Missing or malformed files yield an empty store
    /// (logged as a warn but never fatal — bootstrap must always succeed).
    pub fn load(path: PathBuf) -> Self {
        let packages = match std::fs::read_to_string(&path) {
            Ok(raw) if !raw.trim().is_empty() => match serde_json::from_str::<InstalledPackagesFile>(&raw) {
                Ok(file) => file.packages,
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "installed-packages.json parse failed; starting empty"
                    );
                    Vec::new()
                }
            },
            _ => Vec::new(),
        };
        Self {
            inner: Mutex::new(packages),
            path,
        }
    }

    /// Load from the default path.
    pub fn load_default() -> Self {
        Self::load(Self::default_path())
    }

    /// Snapshot of all persisted records.
    pub fn list(&self) -> Vec<InstalledPackageRecord> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.clone()
    }

    /// Insert-or-replace by `(kind, id)`. Persists immediately.
    pub fn upsert(&self, record: InstalledPackageRecord) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = guard
            .iter_mut()
            .find(|r| r.kind == record.kind && r.id == record.id)
        {
            *existing = record;
        } else {
            guard.push(record);
        }
        self.persist(&guard);
    }

    /// Remove by `(kind, id)`. Returns `true` if the row existed.
    pub fn remove(&self, kind: &PackageKind, id: &str) -> bool {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let before = guard.len();
        guard.retain(|r| !(r.kind == *kind && r.id == id));
        let removed = guard.len() != before;
        if removed {
            self.persist(&guard);
        }
        removed
    }

    fn persist(&self, guard: &[InstalledPackageRecord]) {
        let file = InstalledPackagesFile {
            packages: guard.to_vec(),
        };
        if let Some(parent) = self.path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(
                    path = %parent.display(),
                    error = %e,
                    "installed-packages: failed to create parent dir"
                );
                return;
            }
        }
        match serde_json::to_string_pretty(&file) {
            Ok(s) => {
                if let Err(e) = std::fs::write(&self.path, s) {
                    tracing::warn!(
                        path = %self.path.display(),
                        error = %e,
                        "installed-packages.json write failed"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "installed-packages serialize failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn upsert_and_remove_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("installed-packages.json");
        let store = InstalledPackageStore::load(path.clone());
        assert!(store.list().is_empty());

        store.upsert(InstalledPackageRecord {
            kind: PackageKind::Connector,
            id: "test/local".to_string(),
            source_path: "/tmp/pkg".to_string(),
            version: "0.1.0".to_string(),
        });
        assert_eq!(store.list().len(), 1);

        // Reload to confirm persistence.
        let reloaded = InstalledPackageStore::load(path.clone());
        assert_eq!(reloaded.list().len(), 1);
        assert_eq!(reloaded.list()[0].id, "test/local");

        assert!(store.remove(&PackageKind::Connector, "test/local"));
        assert!(store.list().is_empty());

        let after_remove = InstalledPackageStore::load(path);
        assert!(after_remove.list().is_empty());
    }
}
