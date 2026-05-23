//! Package management HTTP surface (`/api/v2/packages` + `/api/v2/status`).
//!
//! Single source of truth for the platform's 5 ecosystem object kinds
//! (Agent / Skill / Connector / PlatformPlugin / capability count).
//! The CLI (`cyberclaw-cli package list/install/uninstall` and
//! `cyberclaw-cli status`) talks to these endpoints — it no longer keeps
//! a process-local `InMemoryRegistry`.
//!
//! # Routes
//!
//! | Route | Purpose |
//! |---|---|
//! | `GET    /api/v2/packages`                  | List all registered packages (optional `?kind=` filter) |
//! | `POST   /api/v2/packages`                  | Install a package by local manifest path |
//! | `DELETE /api/v2/packages/:kind/:id`        | Uninstall a package |
//! | `GET    /api/v2/status`                    | Aggregate platform counts (agents/skills/connectors/capabilities) |
//!
//! # Persistence
//!
//! Successful installs are recorded in `~/.cyberclaw/installed-packages.json`
//! via [`crate::installed_packages::InstalledPackageStore`]. On server
//! boot, [`bootstrap_user_packages`] re-loads them BEFORE the ecosystem
//! scan so user-installed entries are not clobbered by repo defaults
//! that happen to share an id.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    routing::{delete, get},
    Json, Router,
};
use cyberclaw_control_plane::{
    Loader, ManifestLoader, PackageRecord, PackageSource, Registry, RegistryState,
};
use cyberclaw_core::manifests::PackageKind;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::installed_packages::{InstalledPackageRecord, InstalledPackageStore};
use crate::state::AppState;

/// Build the `/api/v2/{packages,status}` router.
pub fn create_packages_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v2/packages", get(list_packages).post(install_package))
        .route(
            "/api/v2/packages/:kind/:id",
            delete(uninstall_package),
        )
        .route("/api/v2/status", get(get_status))
}

// ----------------------------------------------------------------------
// GET /api/v2/packages
// ----------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// Optional filter: `"agent" | "skill" | "connector" | "plugin"`.
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PackageView {
    pub kind: String,
    pub id: String,
    pub latest_version: String,
    pub active_version: Option<String>,
    pub state: String,
    pub source: String,
    pub summary: String,
    pub capability_count: u32,
}

#[derive(Debug, Serialize)]
pub struct PackagesResponse {
    pub packages: Vec<PackageView>,
    pub total: usize,
}

async fn list_packages(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<PackagesResponse>, ApiError> {
    let kind_filter = q
        .kind
        .as_deref()
        .map(parse_kind_filter)
        .transpose()
        .map_err(ApiError::InvalidRequest)?;

    let records = state
        .package_registry
        .list(kind_filter)
        .await
        .map_err(|e| ApiError::InternalError(format!("registry list failed: {e}")))?;

    let mut packages: Vec<PackageView> = records.iter().map(to_view).collect();
    packages.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.id.cmp(&b.id))
    });
    let total = packages.len();
    Ok(Json(PackagesResponse { packages, total }))
}

// ----------------------------------------------------------------------
// POST /api/v2/packages
// ----------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct InstallRequest {
    /// Local filesystem path to the package directory (containing
    /// `manifest.yaml`).
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct InstallResponse {
    pub kind: String,
    pub id: String,
    pub version: String,
    pub source: String,
}

async fn install_package(
    State(state): State<Arc<AppState>>,
    Json(req): Json<InstallRequest>,
) -> Result<Json<InstallResponse>, ApiError> {
    let trimmed = req.path.trim();
    if trimmed.is_empty() {
        return Err(ApiError::InvalidRequest(
            "`path` must be a non-empty local manifest directory".to_string(),
        ));
    }
    let absolute_path = absolutize_path(trimmed)?;
    if !absolute_path.exists() {
        return Err(ApiError::InvalidRequest(format!(
            "path does not exist on server: {}",
            absolute_path.display()
        )));
    }

    let loader = ManifestLoader::new();
    let source = PackageSource::LocalPath(absolute_path.to_string_lossy().into_owned());
    let loaded = loader
        .load(source.clone())
        .await
        .map_err(|e| ApiError::InvalidRequest(format!("manifest load failed: {e}")))?;

    let record = PackageRecord {
        kind: loaded.manifest.kind.clone(),
        id: loaded.manifest.id.clone(),
        latest_version: loaded.manifest.version.clone(),
        installed_versions: vec![loaded.manifest.version.clone()],
        active_version: Some(loaded.manifest.version.clone()),
        source: source.clone(),
        state: RegistryState::Active,
        available_nodes: Vec::new(),
        runtime_requirements: loaded.manifest.compatibility.runtime.clone(),
        manifest: loaded.manifest.clone(),
    };

    state
        .package_registry
        .upsert(record.clone())
        .await
        .map_err(|e| ApiError::InternalError(format!("registry upsert failed: {e}")))?;
    state
        .package_registry
        .activate(record.kind.clone(), &record.id, &record.latest_version)
        .await
        .map_err(|e| ApiError::InternalError(format!("registry activate failed: {e}")))?;

    state.installed_packages.upsert(InstalledPackageRecord {
        kind: record.kind.clone(),
        id: record.id.clone(),
        source_path: absolute_path.to_string_lossy().into_owned(),
        version: record.latest_version.clone(),
    });

    Ok(Json(InstallResponse {
        kind: kind_str(&record.kind).to_string(),
        id: record.id.clone(),
        version: record.latest_version.clone(),
        source: absolute_path.to_string_lossy().into_owned(),
    }))
}

// ----------------------------------------------------------------------
// DELETE /api/v2/packages/:kind/:id
// ----------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct UninstallResponse {
    pub kind: String,
    pub id: String,
    pub removed: bool,
}

async fn uninstall_package(
    State(state): State<Arc<AppState>>,
    Path((kind_raw, id)): Path<(String, String)>,
) -> Result<Json<UninstallResponse>, ApiError> {
    let kind = parse_kind_filter(&kind_raw).map_err(ApiError::InvalidRequest)?;

    let existing = state
        .package_registry
        .get(kind.clone(), &id)
        .await
        .map_err(|e| ApiError::InternalError(format!("registry get failed: {e}")))?;
    let Some(mut record) = existing else {
        return Err(ApiError::NotFound(format!(
            "package not found: kind={} id={}",
            kind_raw, id
        )));
    };

    record.state = RegistryState::Disabled;
    record.active_version = None;
    state
        .package_registry
        .upsert(record)
        .await
        .map_err(|e| ApiError::InternalError(format!("registry upsert failed: {e}")))?;

    let removed = state.installed_packages.remove(&kind, &id);

    Ok(Json(UninstallResponse {
        kind: kind_str(&kind).to_string(),
        id,
        removed,
    }))
}

// ----------------------------------------------------------------------
// GET /api/v2/status
// ----------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub agents: usize,
    pub skills: usize,
    pub connectors: usize,
    pub plugins: usize,
    pub capabilities: usize,
    pub node_id: String,
    /// LLM model the server will use when a request omits `model`. Sourced
    /// from env (`CYBERCLAW_DEFAULT_MODEL` → `LLM_DEFAULT_MODEL`), falls
    /// back to `"gpt-4"` — same precedence chain as chat_handler.rs's
    /// model resolution. Clients should fetch this at startup so they don't
    /// ship a stale model name from a local cache after admin rotates
    /// `~/.cyberclaw/llm.env`. Discovered 2026-05-19 from real-business
    /// matrix test: chat-tui was reading `~/.cyberclaw/models.json`
    /// (`current_default: deepseek-chat`) and sending that to the server,
    /// while server was running MiniMax — backend returned
    /// `unknown model 'deepseek-chat' (2013)`.
    pub default_model: String,
}

async fn get_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<StatusResponse>, ApiError> {
    // Agents + Plugins come from the manifest-driven package_registry —
    // that's where ecosystem auto-scan + user-installed packages land.
    let agents = state
        .package_registry
        .list(Some(PackageKind::Agent))
        .await
        .map_err(|e| ApiError::InternalError(format!("registry list agents: {e}")))?
        .len();
    let plugins = state
        .package_registry
        .list(Some(PackageKind::PlatformPlugin))
        .await
        .map_err(|e| ApiError::InternalError(format!("registry list plugins: {e}")))?
        .len();

    // Skills surface what the existing `GET /api/v1/skills` route shows —
    // SkillHub-installed bundles (incl. ecosystem/skills/* enumerated at
    // startup), which is what operators expect when they see "skill list"
    // hit 107.
    let skills = {
        let hub = state.skill_hub.read().await;
        hub.list_installed().len()
    };

    // Connectors surface what `GET /api/v1/connectors` shows: the live
    // ConnectorRegistry (sandbox + local.* + memory/todo/MCP/LSP when
    // wired). Falls back to the ship list (9 entries) when registry is
    // empty, mirroring the connectors endpoint's behaviour.
    let connectors = {
        let live = state.connector_registry.list_connectors().len();
        if live == 0 {
            // Same fallback shape as `api::connectors::ship_list().len()`
            // — we don't import the constant to keep this file decoupled.
            9
        } else {
            live
        }
    };

    let capabilities = state.connector_registry.list_capabilities().len();

    let default_model = std::env::var("CYBERCLAW_DEFAULT_MODEL")
        .ok()
        .or_else(|| std::env::var("LLM_DEFAULT_MODEL").ok())
        .unwrap_or_else(|| "gpt-4".to_string());

    Ok(Json(StatusResponse {
        agents,
        skills,
        connectors,
        plugins,
        capabilities,
        node_id: state.node_id.clone(),
        default_model,
    }))
}

// ----------------------------------------------------------------------
// Boot helper
// ----------------------------------------------------------------------

/// Re-apply the user-installed package list to the live registry.
///
/// Called by `main.rs` AFTER `bootstrap_registry_from_ecosystem` so the
/// user's intent overrides the repo defaults when the ids collide
/// (user explicitly installed a newer or different copy).
pub async fn bootstrap_user_packages(
    registry: &Arc<cyberclaw_control_plane::registry::InMemoryRegistry>,
    store: &Arc<InstalledPackageStore>,
) -> anyhow::Result<usize> {
    let records = store.list();
    if records.is_empty() {
        return Ok(0);
    }
    let loader = ManifestLoader::new();
    let mut loaded = 0usize;
    for row in records {
        let path = PathBuf::from(&row.source_path);
        if !path.exists() {
            tracing::warn!(
                path = %path.display(),
                id = %row.id,
                "installed-packages: source path missing on disk, skipping"
            );
            continue;
        }
        let source = PackageSource::LocalPath(row.source_path.clone());
        match loader.load(source.clone()).await {
            Ok(pkg) => {
                let record = PackageRecord {
                    kind: pkg.manifest.kind.clone(),
                    id: pkg.manifest.id.clone(),
                    latest_version: pkg.manifest.version.clone(),
                    installed_versions: vec![pkg.manifest.version.clone()],
                    active_version: Some(pkg.manifest.version.clone()),
                    source,
                    state: RegistryState::Active,
                    available_nodes: Vec::new(),
                    runtime_requirements: pkg.manifest.compatibility.runtime.clone(),
                    manifest: pkg.manifest.clone(),
                };
                if let Err(e) = registry.upsert(record).await {
                    tracing::warn!(error = %e, id = %row.id, "installed-packages: upsert failed");
                    continue;
                }
                loaded += 1;
            }
            Err(e) => {
                tracing::warn!(
                    path = %row.source_path,
                    id = %row.id,
                    error = %e,
                    "installed-packages: manifest reload failed, skipping"
                );
            }
        }
    }
    Ok(loaded)
}

// ----------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------

fn parse_kind_filter(raw: &str) -> Result<PackageKind, String> {
    match raw.to_ascii_lowercase().as_str() {
        "agent" | "agents" => Ok(PackageKind::Agent),
        "skill" | "skills" => Ok(PackageKind::Skill),
        "connector" | "connectors" => Ok(PackageKind::Connector),
        "plugin" | "plugins" | "platformplugin" | "platform_plugin" => {
            Ok(PackageKind::PlatformPlugin)
        }
        other => Err(format!(
            "invalid kind '{other}' (expected agent|skill|connector|plugin)"
        )),
    }
}

fn kind_str(kind: &PackageKind) -> &'static str {
    match kind {
        PackageKind::Agent => "agent",
        PackageKind::Skill => "skill",
        PackageKind::Connector => "connector",
        PackageKind::PlatformPlugin => "plugin",
    }
}

fn source_str(source: &PackageSource) -> String {
    match source {
        PackageSource::LocalPath(p) => format!("local:{p}"),
        PackageSource::Registry(r) => format!("registry:{r}"),
        PackageSource::Git(g) => format!("git:{g}"),
        PackageSource::Archive(a) => format!("archive:{a}"),
    }
}

fn state_str(state: &RegistryState) -> &'static str {
    match state {
        RegistryState::Discovered => "discovered",
        RegistryState::Installed => "installed",
        RegistryState::Validated => "validated",
        RegistryState::Active => "active",
        RegistryState::Disabled => "disabled",
        RegistryState::Failed => "failed",
    }
}

fn capability_count(record: &PackageRecord) -> u32 {
    match &record.manifest.spec {
        cyberclaw_core::manifests::PackageSpec::Connector(spec) => spec.capabilities.len() as u32,
        _ => 0,
    }
}

fn to_view(record: &PackageRecord) -> PackageView {
    PackageView {
        kind: kind_str(&record.kind).to_string(),
        id: record.id.clone(),
        latest_version: record.latest_version.clone(),
        active_version: record.active_version.clone(),
        state: state_str(&record.state).to_string(),
        source: source_str(&record.source),
        summary: record.manifest.summary.clone(),
        capability_count: capability_count(record),
    }
}

fn absolutize_path(raw: &str) -> Result<PathBuf, ApiError> {
    let p = PathBuf::from(raw);
    let absolute = if p.is_absolute() {
        p
    } else {
        std::env::current_dir()
            .map_err(|e| ApiError::InternalError(format!("cwd unavailable: {e}")))?
            .join(p)
    };
    Ok(absolute)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kind_filter_accepts_singular_and_plural() {
        assert!(matches!(
            parse_kind_filter("agent").unwrap(),
            PackageKind::Agent
        ));
        assert!(matches!(
            parse_kind_filter("plugins").unwrap(),
            PackageKind::PlatformPlugin
        ));
        assert!(parse_kind_filter("foo").is_err());
    }

    #[test]
    fn kind_str_round_trips() {
        for k in [
            PackageKind::Agent,
            PackageKind::Skill,
            PackageKind::Connector,
            PackageKind::PlatformPlugin,
        ] {
            let s = kind_str(&k);
            let parsed = parse_kind_filter(s).unwrap();
            assert_eq!(parsed, k);
        }
    }
}
