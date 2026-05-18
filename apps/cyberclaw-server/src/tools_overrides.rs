//! Persistent tool promote/demote overrides.
//!
//! 2026-05-17 (v1.2.8): `cyberclaw-cli tools promote <name>` used to update
//! only the in-memory `DeferredToolRegistry`. Restart re-ran
//! `with_defaults()` and threw away every operator override — `browser_evaluate`
//! flipped back to `deferred` every boot, breaking the LLM tool palette
//! the operator had just curated.
//!
//! Fix: read/write `~/.cyberclaw/tools.json` whenever promote/demote
//! mutates registry state. State.rs applies the file on top of the default
//! seeding immediately after `DeferredToolRegistry::new(50)` finishes.
//!
//! File format (newline-pretty JSON for human edit-ability):
//! ```json
//! {
//!   "browser_evaluate": "active",
//!   "browser_dialog_handle": "active",
//!   "some_other_tool": "deferred"
//! }
//! ```
//!
//! Missing file = no overrides (= default seeding stands). Parse errors log
//! warn and skip; server still boots clean.

use cyberclaw_agent_runtime::deferred_registry::DeferredToolRegistry;
use std::collections::HashMap;
use std::path::PathBuf;

/// Override value — either `"active"` or `"deferred"`. Unknown values are
/// skipped with a warn at load time (forward-compat with future states).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverrideState {
    Active,
    Deferred,
}

impl OverrideState {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "deferred" => Some(Self::Deferred),
            _ => None,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Deferred => "deferred",
        }
    }
}

/// File path for persisted overrides. Uses `$HOME/.cyberclaw/tools.json`.
/// Falls back to `./.cyberclaw/tools.json` if `HOME` is unset (unusual but
/// keeps tests / containers without a home dir functional).
fn overrides_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".cyberclaw").join("tools.json")
}

/// Read overrides from disk. Returns empty map if file missing or unreadable.
pub fn load_overrides() -> HashMap<String, OverrideState> {
    let path = overrides_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return HashMap::new(),
    };
    let parsed: HashMap<String, String> = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "tools.json parse failed; ignoring all persisted overrides"
            );
            return HashMap::new();
        }
    };
    let mut out = HashMap::with_capacity(parsed.len());
    for (name, state) in parsed {
        match OverrideState::parse(&state) {
            Some(s) => {
                out.insert(name, s);
            }
            None => {
                tracing::warn!(
                    tool = %name,
                    state = %state,
                    "tools.json contains unknown state; skipping this override"
                );
            }
        }
    }
    out
}

/// Apply overrides on top of an already-seeded registry. Tools listed in
/// the overrides map get flipped to the requested state via the same
/// `promote()` / `demote()` paths the HTTP handlers use, so any side-effects
/// (audit, idempotency) stay consistent.
pub fn apply_overrides(registry: &mut DeferredToolRegistry) -> usize {
    let overrides = load_overrides();
    let mut applied = 0usize;
    for (name, state) in &overrides {
        let ok = match state {
            OverrideState::Active => registry.promote(name),
            OverrideState::Deferred => registry.demote(name),
        };
        if ok {
            applied += 1;
        } else {
            tracing::warn!(
                tool = %name,
                requested = %state.as_str(),
                "tools.json names a tool not in the registry; skipping"
            );
        }
    }
    if applied > 0 {
        tracing::info!(
            count = applied,
            "applied persisted tool overrides from tools.json"
        );
    }
    applied
}

/// Save a single tool's new state to disk via atomic write (NamedTempFile +
/// persist). Read-modify-write so we don't clobber other overrides set in
/// the same session. Best-effort: log warn and return Err on IO failure —
/// the runtime promote/demote already succeeded, the persistence layer
/// should never block the API success.
///
/// **Concurrency note** (v1.2.9): the RMW is NOT under an exclusive file
/// lock. Two concurrent admin-level promotes can race — both read the
/// pre-state, both append their own entry, the later rename wins and the
/// earlier writer's update is silently lost. Acceptable for admin tooling
/// (single operator, low frequency); upgrade to `fs2::FileExt::lock_exclusive`
/// or a state-store-backed key/value if real concurrency arrives. The
/// atomic rename below at least guarantees no half-written file (concurrent
/// writes don't observe corruption, just last-write-wins).
pub fn save_override(name: &str, state: OverrideState) -> std::io::Result<()> {
    let path = overrides_path();
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent dir"))?;
    if !parent.exists() {
        std::fs::create_dir_all(parent)?;
    }
    let mut current = load_overrides();
    current.insert(name.to_string(), state);
    // Convert to String map for stable serde output.
    let serializable: HashMap<&str, &str> = current
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let body = serde_json::to_string_pretty(&serializable).map_err(std::io::Error::other)?;

    // Atomic write — write to temp file in same dir, then rename. Rename is
    // POSIX-atomic on the same filesystem, so readers either see the old
    // file or the fully-written new one, never a half-written one.
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    std::io::Write::write_all(&mut tmp, body.as_bytes())?;
    std::io::Write::flush(&mut tmp)?;
    tmp.persist(&path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::Mutex;

    static HOME_LOCK: Mutex<()> = Mutex::new(());

    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var("HOME").ok();
        // SAFETY: set_var/remove_var are unsafe in Rust 2024 only via the
        // edition-marker; here we're on edition 2021 + tests are serial
        // (HOME_LOCK + #[serial]), so single-threaded mutation is OK.
        std::env::set_var("HOME", tmp.path());
        f();
        if let Some(v) = prev {
            std::env::set_var("HOME", v);
        } else {
            std::env::remove_var("HOME");
        }
    }

    #[test]
    fn parse_round_trip() {
        for s in [OverrideState::Active, OverrideState::Deferred] {
            assert_eq!(OverrideState::parse(s.as_str()), Some(s));
        }
        assert_eq!(OverrideState::parse("hidden"), None);
        assert_eq!(OverrideState::parse(""), None);
    }

    #[test]
    #[serial]
    fn save_then_load_round_trip() {
        with_temp_home(|| {
            assert!(load_overrides().is_empty(), "fresh HOME = empty overrides");
            save_override("browser_evaluate", OverrideState::Active).expect("save 1");
            save_override("file_read", OverrideState::Deferred).expect("save 2");
            save_override("browser_dialog_handle", OverrideState::Active).expect("save 3");
            let loaded = load_overrides();
            assert_eq!(loaded.len(), 3, "three saves → three entries");
            assert_eq!(loaded.get("browser_evaluate"), Some(&OverrideState::Active));
            assert_eq!(loaded.get("file_read"), Some(&OverrideState::Deferred));
            assert_eq!(
                loaded.get("browser_dialog_handle"),
                Some(&OverrideState::Active)
            );
        });
    }

    #[test]
    #[serial]
    fn save_overwrites_existing_entry() {
        with_temp_home(|| {
            save_override("X", OverrideState::Active).unwrap();
            save_override("X", OverrideState::Deferred).unwrap();
            let loaded = load_overrides();
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded.get("X"), Some(&OverrideState::Deferred));
        });
    }

    #[test]
    #[serial]
    fn corrupt_file_degrades_to_empty() {
        with_temp_home(|| {
            let path = overrides_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "this is not json {{{").unwrap();
            // load_overrides should return empty, not panic
            let loaded = load_overrides();
            assert!(
                loaded.is_empty(),
                "corrupt tools.json must degrade to empty overrides, not crash boot"
            );
        });
    }
}
