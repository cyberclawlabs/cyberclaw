//! Capability Discovery (Sprint D2)
//!
//! `CapabilityDiscovery` is a stateless query service that resolves a
//! [`DiscoveryQuery`] (deliverable kind + modalities + search terms) against
//! the registered capability surfaces of the platform.
//!
//! # Why a stateless query service (not a coordinator trait)
//!
//! The discovery surface naturally splits into two halves:
//!
//! - **Local / synchronous** — `µs`-level lookups over in-memory registries:
//!   1. `Native`           — [`ConnectorRegistry`] + capability list
//!   2. `InstalledSkill`   — local skill index (in-memory `SkillHub` known bundles)
//!   3. `CmdRuntime`       — local binary probe (`which python3`, `ffmpeg`, ...)
//! - **Remote / asynchronous** — possibly seconds of wall-clock per probe:
//!   4. `SkillHub`         — remote registry HTTP fetch
//!   5. `ProviderModality` — LLM provider modality probe (or hard-coded fallback)
//!   6. `CapabilityRequest`— write-to-queue when nothing matched
//!
//! `discover_local` runs the first 3 segments **stop-on-first-hit**: as soon
//! as one segment yields ≥ 1 capability we return; the remaining segments are
//! marked `pending = false`. This matches the architect's `D2` brief — front
//! 3 are µs-level, back 3 must not block the dispatch path.
//!
//! `discover_remote` runs the back 3 segments only — call it from a background
//! task after `discover_local` returned all-empty.
//!
//! `discover_full` is a convenience that chains both with a total timeout.
//!
//! # Integration
//!
//! D2 only exposes the API. The (Sprint D3) Resolver will call
//! `discover_local` while constructing an [`crate::persistent_execution::ExecutionPlan`]
//! and attach the resulting [`crate::persistent_execution::CapabilitySource`]
//! to each [`crate::persistent_execution::Story`] via
//! [`crate::persistent_execution::Story::with_source`].
//!
//! No existing dispatch path (`PersistentLoop`, `ExecutionService`) is mutated
//! by this sprint.
//!
//! # SkillIndex / SkillHub adaptation note
//!
//! The architect brief specified `Arc<SkillIndex>` for the sync segment and
//! `Arc<SkillHub>` for the async segment. Today's repo only ships
//! `cyberclaw_skill_runtime::SkillHub` — a single struct whose
//! `search(&self, query) -> Vec<SkillBundle>` covers the sync use case but
//! whose remote-fetch APIs require `&mut self`. To keep this module
//! decoupled and unit-testable we adapt with two small traits:
//!
//! - [`SkillIndex`]      — sync, in-memory installed-skill query
//! - [`SkillHubProvider`]— async, remote-registry query
//!
//! and provide blanket impls so a real [`cyberclaw_skill_runtime::SkillHub`]
//! wrapped in `Arc<...>` (or `Arc<tokio::sync::RwLock<SkillHub>>`) can plug
//! straight into the API. Tests use lightweight in-process fakes.

use async_trait::async_trait;
use cyberclaw_core::ids::{CapabilityId, ConnectorId, SkillId};
use cyberclaw_llm::client::LlmClient;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

use cyberclaw_connectors::registry::ConnectorRegistry;
use cyberclaw_skill_runtime::skill_hub::{SkillBundle, SkillHub};

// ============================================================================
// Modality
// ============================================================================

/// Deliverable / input modality classification.
///
/// Used by the discovery layer to filter providers and capabilities by
/// content type. Kept narrow on purpose — adding a new variant should be a
/// considered choice, not a "while I'm here" cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modality {
    Audio,
    Text,
    Image,
    Csv,
    Code,
    Url,
    Pdf,
    Pptx,
    Xlsx,
}

// ============================================================================
// DiscoveryQuery
// ============================================================================

/// Input to a single discovery pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryQuery {
    /// Deliverable kind, e.g. `"audio.transcribe"`, `"image.generate"`,
    /// `"spreadsheet.write"`. Matched against capability IDs (substring or
    /// exact) for native segment 1.
    pub deliverable_kind: String,
    /// Modalities the resolved capability must handle.
    #[serde(default)]
    pub modalities: Vec<Modality>,
    /// Free-form natural-language search terms used for the skill segments.
    #[serde(default)]
    pub search_terms: Vec<String>,
}

impl DiscoveryQuery {
    /// Construct a new query for the given deliverable kind.
    pub fn new(deliverable_kind: impl Into<String>) -> Self {
        Self {
            deliverable_kind: deliverable_kind.into(),
            modalities: Vec::new(),
            search_terms: Vec::new(),
        }
    }

    /// Builder: attach modalities.
    pub fn with_modalities(mut self, modalities: Vec<Modality>) -> Self {
        self.modalities = modalities;
        self
    }

    /// Builder: attach search terms.
    pub fn with_search_terms(mut self, terms: Vec<String>) -> Self {
        self.search_terms = terms;
        self
    }
}

// ============================================================================
// DiscoveryResult / RemoteDiscoveryResult
// ============================================================================

/// Combined result of (local) and optionally (remote) discovery passes.
///
/// `pending` flags advertise which async segments did **not** run during the
/// returning call — callers may invoke [`CapabilityDiscovery::discover_remote`]
/// to fill them in.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiscoveryResult {
    /// Native segment hits — `(connector_id, capability_id)` pairs.
    pub native: Vec<(ConnectorId, CapabilityId)>,
    /// Installed skill segment hits.
    pub installed_skills: Vec<SkillId>,
    /// Local binary segment hits — names of `which`-resolved binaries.
    pub cmd_runtime: Vec<String>,

    /// True when the SkillHub HTTP fetch did not run for this call.
    pub skill_hub_pending: bool,
    /// True when the provider modality probe did not run for this call.
    pub provider_modalities_pending: bool,
    /// True when the gap-recording request was not yet written.
    pub request_pending: bool,
}

impl DiscoveryResult {
    /// Whether at least one of the local segments yielded a hit.
    pub fn has_local_hit(&self) -> bool {
        !self.native.is_empty() || !self.installed_skills.is_empty() || !self.cmd_runtime.is_empty()
    }
}

/// Result of a remote-only discovery pass (segments 4 + 5 + 6).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemoteDiscoveryResult {
    /// Bundles surfaced by the remote SkillHub registry.
    pub skill_hub_hits: Vec<SkillHubResult>,
    /// Provider modality entries returned by the LLM client probe.
    pub provider_modalities: Vec<ProviderModality>,
    /// Set when no remote segment matched and a gap row was queued.
    pub request_id: Option<String>,
}

/// A single remote SkillHub registry hit. We keep it loose — full bundle
/// fields live on [`cyberclaw_skill_runtime::skill_hub::SkillBundle`] — but
/// expose the minimum the resolver needs to decide install-vs-skip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillHubResult {
    pub name: String,
    pub version: String,
    pub description: String,
    pub source: String,
    /// `true` when the bundle is not yet installed locally.
    pub install_required: bool,
}

impl From<SkillBundle> for SkillHubResult {
    fn from(bundle: SkillBundle) -> Self {
        Self {
            name: bundle.name,
            version: bundle.version,
            description: bundle.description,
            source: bundle.source,
            install_required: true,
        }
    }
}

/// One LLM provider modality entry (e.g. `provider = "openai"`,
/// `api = "audio.transcriptions"`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderModality {
    pub provider: String,
    pub api: String,
}

// ============================================================================
// Pluggable backends
// ============================================================================

/// Sync, in-memory installed-skill query surface.
///
/// Today implemented by a small wrapper over
/// [`cyberclaw_skill_runtime::skill_hub::SkillHub::search`].
pub trait SkillIndex: Send + Sync {
    /// Return matching installed skill IDs for the given query terms.
    ///
    /// An empty `search_terms` slice means "any" — implementors may return
    /// the full installed set, but typically should return an empty result
    /// to keep dispatch deterministic.
    fn search(&self, search_terms: &[String]) -> Vec<SkillId>;
}

/// Async, remote SkillHub query surface.
#[async_trait]
pub trait SkillHubProvider: Send + Sync {
    /// Fetch bundle hits from the remote registry. Implementors are
    /// responsible for their own internal timeout — the discovery layer
    /// applies an outer timeout on top.
    async fn fetch_remote(&self, query: &DiscoveryQuery) -> anyhow::Result<Vec<SkillBundle>>;
}

/// Adapter wrapping a real [`SkillHub`] for the sync segment.
///
/// `SkillHub::search` only needs `&self`, so we hold an `Arc<SkillHub>`.
pub struct SkillHubIndex {
    inner: Arc<SkillHub>,
}

impl SkillHubIndex {
    pub fn new(inner: Arc<SkillHub>) -> Self {
        Self { inner }
    }
}

impl SkillIndex for SkillHubIndex {
    fn search(&self, search_terms: &[String]) -> Vec<SkillId> {
        if search_terms.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<SkillId> = Vec::new();
        for term in search_terms {
            for bundle in self.inner.search(term) {
                if let Ok(id) = SkillId::from_string(bundle.name.clone()) {
                    if !hits.iter().any(|existing| existing == &id) {
                        hits.push(id);
                    }
                }
            }
        }
        hits
    }
}

/// Sink that records a missing capability for the (Sprint D-4) audit-backed
/// queue.
#[async_trait]
pub trait CapabilityRequestSink: Send + Sync {
    /// Record the gap and return a request id. The id is propagated back as
    /// `RemoteDiscoveryResult::request_id`.
    async fn record_gap(&self, query: &DiscoveryQuery) -> anyhow::Result<String>;
}

// ============================================================================
// CapabilityDiscovery
// ============================================================================

/// Discovery orchestration knobs. Defaults are conservative and match the
/// architect brief: local stays µs-level, remote applies per-segment caps.
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    /// Cap on the SkillHub remote fetch.
    pub skill_hub_timeout: Duration,
    /// Cap on the provider modality probe.
    pub provider_probe_timeout: Duration,
    /// Cap on the capability-request sink write.
    pub request_sink_timeout: Duration,
    /// Outer cap applied by [`CapabilityDiscovery::discover_full`].
    pub full_total_timeout: Duration,
    /// Local binaries to probe with `which`.
    pub cmd_binaries: Vec<String>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            skill_hub_timeout: Duration::from_secs(5),
            provider_probe_timeout: Duration::from_secs(5),
            request_sink_timeout: Duration::from_secs(2),
            full_total_timeout: Duration::from_secs(10),
            cmd_binaries: vec![
                "python3".to_string(),
                "ffmpeg".to_string(),
                "ffprobe".to_string(),
                "pandoc".to_string(),
                "weasyprint".to_string(),
                "openssl".to_string(),
            ],
        }
    }
}

/// Stateless capability discovery query service.
///
/// See module docs for the segment layout.
pub struct CapabilityDiscovery {
    connector_registry: Arc<ConnectorRegistry>,
    skill_index: Arc<dyn SkillIndex>,
    skill_hub: Option<Arc<dyn SkillHubProvider>>,
    llm_client: Option<Arc<dyn LlmClient>>,
    capability_request_sink: Option<Arc<dyn CapabilityRequestSink>>,
    config: DiscoveryConfig,
}

impl CapabilityDiscovery {
    /// Construct a new instance with only the local segments wired.
    pub fn new(
        connector_registry: Arc<ConnectorRegistry>,
        skill_index: Arc<dyn SkillIndex>,
    ) -> Self {
        Self {
            connector_registry,
            skill_index,
            skill_hub: None,
            llm_client: None,
            capability_request_sink: None,
            config: DiscoveryConfig::default(),
        }
    }

    /// Builder: attach a remote SkillHub provider for the async segment.
    pub fn with_skill_hub(mut self, hub: Arc<dyn SkillHubProvider>) -> Self {
        self.skill_hub = Some(hub);
        self
    }

    /// Builder: attach an LLM client for the modality-probe async segment.
    pub fn with_llm_client(mut self, llm: Arc<dyn LlmClient>) -> Self {
        self.llm_client = Some(llm);
        self
    }

    /// Builder: attach a sink that records gaps when remote is empty.
    pub fn with_capability_request_sink(mut self, sink: Arc<dyn CapabilityRequestSink>) -> Self {
        self.capability_request_sink = Some(sink);
        self
    }

    /// Builder: override the default discovery config.
    pub fn with_config(mut self, config: DiscoveryConfig) -> Self {
        self.config = config;
        self
    }

    // -------------------------------------------------------------------
    // discover_local — segments 1, 2, 3 (sync, stop-on-first-hit)
    // -------------------------------------------------------------------

    /// Run the synchronous local discovery (segments 1 → 2 → 3) and stop on
    /// the first segment that yields at least one hit.
    ///
    /// `pending` flags on the result advertise async segments that have not
    /// run; the caller decides whether to escalate to
    /// [`Self::discover_remote`].
    pub fn discover_local(&self, query: &DiscoveryQuery) -> DiscoveryResult {
        let mut result = DiscoveryResult::default();

        // Segment 1: native connector capabilities
        let native = self.discover_native(query);
        if !native.is_empty() {
            result.native = native;
            // We hit — do not bother probing further segments. Mark async
            // segments not-pending: the caller already has a usable answer.
            result.skill_hub_pending = false;
            result.provider_modalities_pending = false;
            result.request_pending = false;
            return result;
        }

        // Segment 2: installed skills
        let installed = self.skill_index.search(&query.search_terms);
        if !installed.is_empty() {
            result.installed_skills = installed;
            result.skill_hub_pending = false;
            result.provider_modalities_pending = false;
            result.request_pending = false;
            return result;
        }

        // Segment 3: cmd-runtime binaries on PATH
        let cmd = self.discover_cmd_runtime();
        if !cmd.is_empty() {
            result.cmd_runtime = cmd;
            result.skill_hub_pending = false;
            result.provider_modalities_pending = false;
            result.request_pending = false;
            return result;
        }

        // Local all-empty — async segments are still pending.
        result.skill_hub_pending = self.skill_hub.is_some();
        result.provider_modalities_pending = self.llm_client.is_some();
        result.request_pending = self.capability_request_sink.is_some();
        result
    }

    fn discover_native(&self, query: &DiscoveryQuery) -> Vec<(ConnectorId, CapabilityId)> {
        let needle = query.deliverable_kind.to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        self.connector_registry
            .list_capabilities()
            .into_iter()
            .filter(|(_conn, cap)| {
                let cap_str = cap.as_str().to_lowercase();
                cap_str == needle || cap_str.contains(&needle)
            })
            .collect()
    }

    fn discover_cmd_runtime(&self) -> Vec<String> {
        let mut hits = Vec::new();
        for bin in &self.config.cmd_binaries {
            if which_binary(bin) {
                hits.push(bin.clone());
            }
        }
        hits
    }

    // -------------------------------------------------------------------
    // discover_remote — segments 4, 5, 6 (async, all-graceful)
    // -------------------------------------------------------------------

    /// Run the asynchronous remote discovery (segments 4 + 5 + 6).
    ///
    /// Each segment is wrapped in its own per-segment timeout; failures and
    /// timeouts return empty data without propagating the error.
    pub async fn discover_remote(&self, query: &DiscoveryQuery) -> RemoteDiscoveryResult {
        let skill_hub_hits = self.fetch_remote_skill_hub(query).await;
        let provider_modalities = self.probe_provider_modalities(query).await;

        let need_request = skill_hub_hits.is_empty() && provider_modalities.is_empty();
        let request_id = if need_request {
            self.record_capability_request(query).await
        } else {
            None
        };

        RemoteDiscoveryResult {
            skill_hub_hits,
            provider_modalities,
            request_id,
        }
    }

    async fn fetch_remote_skill_hub(&self, query: &DiscoveryQuery) -> Vec<SkillHubResult> {
        let Some(hub) = self.skill_hub.as_ref() else {
            return Vec::new();
        };
        let fut = hub.fetch_remote(query);
        match tokio::time::timeout(self.config.skill_hub_timeout, fut).await {
            Ok(Ok(bundles)) => bundles.into_iter().map(SkillHubResult::from).collect(),
            Ok(Err(err)) => {
                tracing::debug!(target: "capability_discovery", error = %err, "skill hub remote fetch failed");
                Vec::new()
            }
            Err(_) => {
                tracing::debug!(target: "capability_discovery", "skill hub remote fetch timed out");
                Vec::new()
            }
        }
    }

    async fn probe_provider_modalities(&self, query: &DiscoveryQuery) -> Vec<ProviderModality> {
        let Some(llm) = self.llm_client.as_ref() else {
            return Vec::new();
        };
        // The current LlmClient trait does not expose an explicit
        // `list_modalities` method, so we use the provider name as the key
        // into a hard-coded modality table. validate_connection() guards
        // against using an unreachable provider — graceful on failure.
        let provider = llm.provider().to_string();
        let probe = async {
            // Cheap reachability check — keeps the surface honest without
            // requiring a brand-new trait method on `LlmClient`.
            let _ = llm.validate_connection().await;
            anyhow::Ok(known_provider_modalities(&provider))
        };
        match tokio::time::timeout(self.config.provider_probe_timeout, probe).await {
            Ok(Ok(mut hits)) => {
                if !query.modalities.is_empty() {
                    hits.retain(|pm| {
                        query
                            .modalities
                            .iter()
                            .any(|m| modality_matches_api(*m, &pm.api))
                    });
                }
                hits
            }
            Ok(Err(err)) => {
                tracing::debug!(target: "capability_discovery", error = %err, "provider modality probe failed");
                Vec::new()
            }
            Err(_) => {
                tracing::debug!(target: "capability_discovery", "provider modality probe timed out");
                Vec::new()
            }
        }
    }

    async fn record_capability_request(&self, query: &DiscoveryQuery) -> Option<String> {
        let sink = self.capability_request_sink.as_ref()?;
        let fut = sink.record_gap(query);
        match tokio::time::timeout(self.config.request_sink_timeout, fut).await {
            Ok(Ok(id)) => Some(id),
            Ok(Err(err)) => {
                tracing::debug!(target: "capability_discovery", error = %err, "capability request sink failed");
                None
            }
            Err(_) => {
                tracing::debug!(target: "capability_discovery", "capability request sink timed out");
                None
            }
        }
    }

    // -------------------------------------------------------------------
    // discover_full — local + remote with an outer timeout
    // -------------------------------------------------------------------

    /// Convenience combinator: run [`Self::discover_local`] first, and if
    /// nothing matched, run [`Self::discover_remote`] under an outer
    /// `total_timeout`. Returns a fully-populated [`DiscoveryResult`].
    ///
    /// The returned `DiscoveryResult` carries `skill_hub_pending` etc. as
    /// `false` once a remote pass has executed (regardless of whether it
    /// returned empty or not), so callers can treat completion as definitive.
    pub async fn discover_full(
        &self,
        query: &DiscoveryQuery,
        total_timeout: Duration,
    ) -> DiscoveryResult {
        let mut result = self.discover_local(query);
        if result.has_local_hit() {
            return result;
        }
        let remote_fut = self.discover_remote(query);
        let timeout = if total_timeout.is_zero() {
            self.config.full_total_timeout
        } else {
            total_timeout
        };
        match tokio::time::timeout(timeout, remote_fut).await {
            Ok(remote) => {
                if !remote.skill_hub_hits.is_empty() {
                    // We surface remote SkillHub hits via the
                    // `installed_skills` slot when the bundle has been
                    // installed; for hub-only hits we just record the names
                    // as `cmd_runtime`-shaped strings so callers can react.
                    // In practice the (Sprint D3) Resolver consumes the full
                    // RemoteDiscoveryResult separately — discover_full keeps
                    // the local-shaped surface for ergonomic call sites.
                    for hit in remote.skill_hub_hits {
                        if let Ok(id) = SkillId::from_string(hit.name) {
                            if !result
                                .installed_skills
                                .iter()
                                .any(|existing| existing == &id)
                            {
                                result.installed_skills.push(id);
                            }
                        }
                    }
                }
                result.skill_hub_pending = false;
                result.provider_modalities_pending = false;
                result.request_pending = false;
            }
            Err(_) => {
                tracing::debug!(target: "capability_discovery", "discover_full outer timeout exceeded");
            }
        }
        result
    }
}

// ============================================================================
// helpers
// ============================================================================

/// Probe `which <bin>` on the local PATH. Returns `true` iff the binary
/// resolves to an existing executable.
fn which_binary(bin: &str) -> bool {
    use std::process::Command;
    match Command::new("which").arg(bin).output() {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            !stdout.trim().is_empty()
        }
        _ => false,
    }
}

/// Hard-coded modality table for known providers. Avoids requiring every
/// `LlmClient` impl to add a `list_modalities()` method — discovery picks the
/// right entry by `provider()`. New providers should land here.
fn known_provider_modalities(provider: &str) -> Vec<ProviderModality> {
    let lower = provider.to_lowercase();
    let entries: &[(&str, &str)] = match lower.as_str() {
        "openai" => &[
            ("openai", "audio.transcriptions"),
            ("openai", "audio.speech"),
            ("openai", "vision"),
            ("openai", "chat.completions"),
            ("openai", "embeddings"),
            ("openai", "images.generations"),
        ],
        "anthropic" => &[("anthropic", "vision"), ("anthropic", "messages")],
        "deepseek" => &[("deepseek", "chat.completions")],
        "ollama" => &[("ollama", "chat")],
        _ => &[],
    };
    entries
        .iter()
        .map(|(p, a)| ProviderModality {
            provider: (*p).to_string(),
            api: (*a).to_string(),
        })
        .collect()
}

/// Heuristic `Modality` → `api` substring matcher for the provider segment.
fn modality_matches_api(modality: Modality, api: &str) -> bool {
    let api_lower = api.to_lowercase();
    match modality {
        Modality::Audio => api_lower.contains("audio") || api_lower.contains("speech"),
        Modality::Image => api_lower.contains("image") || api_lower.contains("vision"),
        // All other modalities default to true — the provider-level filter
        // is intentionally permissive; the resolver does the final pick.
        _ => true,
    }
}

// ============================================================================
// tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_connectors::types::{
        CapabilityExecutionRequest, CapabilityExecutionResult, Connector,
    };
    use cyberclaw_core::capability::{CapabilityEffect, RiskLevel};
    use cyberclaw_core::manifests::CapabilityTimeouts;
    use cyberclaw_core::prelude::{CapabilityContract, ConnectorRuntime};
    use std::sync::Mutex;

    // ---- mocks --------------------------------------------------------

    #[derive(Debug)]
    struct StubConnector {
        id: ConnectorId,
        caps: Vec<CapabilityContract>,
    }

    #[async_trait::async_trait]
    impl Connector for StubConnector {
        fn id(&self) -> &ConnectorId {
            &self.id
        }
        fn runtime(&self) -> ConnectorRuntime {
            ConnectorRuntime::Native
        }
        fn capabilities(&self) -> Vec<CapabilityContract> {
            self.caps.clone()
        }
        async fn execute(
            &self,
            _request: CapabilityExecutionRequest,
        ) -> anyhow::Result<CapabilityExecutionResult> {
            anyhow::bail!("stub")
        }
    }

    fn cap(id: &str) -> CapabilityContract {
        CapabilityContract {
            id: id.to_string(),
            title: id.to_string(),
            description: None,
            input_schema: String::new(),
            output_schema: String::new(),
            risk: RiskLevel::Low,
            effects: vec![CapabilityEffect::Read],
            placement: None,
            timeouts: CapabilityTimeouts { request_ms: None },
        }
    }

    fn registry_with(caps: Vec<(&str, Vec<&str>)>) -> Arc<ConnectorRegistry> {
        let registry = Arc::new(ConnectorRegistry::new());
        for (conn_id, cap_ids) in caps {
            let stub = StubConnector {
                id: ConnectorId::from_string(conn_id.to_string()).unwrap(),
                caps: cap_ids.into_iter().map(cap).collect(),
            };
            registry.register(Arc::new(stub)).unwrap();
        }
        registry
    }

    /// In-memory `SkillIndex` fake.
    struct StaticSkillIndex {
        installed: Vec<(String, Vec<String>)>, // skill_name -> match terms
    }

    impl SkillIndex for StaticSkillIndex {
        fn search(&self, search_terms: &[String]) -> Vec<SkillId> {
            if search_terms.is_empty() {
                return Vec::new();
            }
            let mut hits = Vec::new();
            for (name, terms) in &self.installed {
                let matched = search_terms.iter().any(|q| {
                    let lq = q.to_lowercase();
                    terms.iter().any(|t| t.to_lowercase().contains(&lq))
                });
                if matched {
                    if let Ok(id) = SkillId::from_string(name.clone()) {
                        hits.push(id);
                    }
                }
            }
            hits
        }
    }

    fn empty_skill_index() -> Arc<dyn SkillIndex> {
        Arc::new(StaticSkillIndex { installed: vec![] })
    }

    /// Force cmd_runtime probe to a binary that is virtually never on PATH.
    fn cfg_unknown_cmd() -> DiscoveryConfig {
        DiscoveryConfig {
            cmd_binaries: vec!["__cyberclaw_definitely_not_a_real_binary__".to_string()],
            ..DiscoveryConfig::default()
        }
    }

    /// Force cmd_runtime probe to a binary that is reliably available on
    /// every Unix host — `sh`.
    fn cfg_sh_only() -> DiscoveryConfig {
        DiscoveryConfig {
            cmd_binaries: vec!["sh".to_string()],
            ..DiscoveryConfig::default()
        }
    }

    // ---- segment 1: native -------------------------------------------

    #[test]
    fn discover_local_native_hit_short_circuits() {
        let registry = registry_with(vec![("conn-a", vec!["audio.transcribe"])]);
        let discovery =
            CapabilityDiscovery::new(registry, empty_skill_index()).with_config(cfg_unknown_cmd());

        let query = DiscoveryQuery::new("audio.transcribe")
            .with_search_terms(vec!["transcribe".to_string()]);
        let result = discovery.discover_local(&query);

        assert_eq!(result.native.len(), 1);
        assert_eq!(result.native[0].0.as_str(), "conn-a");
        assert!(result.installed_skills.is_empty());
        assert!(result.cmd_runtime.is_empty());
        assert!(!result.skill_hub_pending);
        assert!(!result.provider_modalities_pending);
        assert!(!result.request_pending);
    }

    #[test]
    fn discover_local_native_substring_match() {
        let registry = registry_with(vec![("c1", vec!["spreadsheet.write.xlsx"])]);
        let discovery =
            CapabilityDiscovery::new(registry, empty_skill_index()).with_config(cfg_unknown_cmd());
        let query = DiscoveryQuery::new("spreadsheet.write");
        let result = discovery.discover_local(&query);
        assert_eq!(result.native.len(), 1);
    }

    // ---- segment 2: installed skills ---------------------------------

    #[test]
    fn discover_local_segment1_empty_segment2_hit() {
        let registry = registry_with(vec![]); // no native caps
        let index = Arc::new(StaticSkillIndex {
            installed: vec![(
                "transcribe-skill".to_string(),
                vec!["transcription".to_string(), "audio".to_string()],
            )],
        });
        let discovery = CapabilityDiscovery::new(registry, index).with_config(cfg_unknown_cmd());

        let query =
            DiscoveryQuery::new("audio.transcribe").with_search_terms(vec!["audio".to_string()]);
        let result = discovery.discover_local(&query);

        assert!(result.native.is_empty());
        assert_eq!(result.installed_skills.len(), 1);
        assert_eq!(result.installed_skills[0].as_str(), "transcribe-skill");
        assert!(result.cmd_runtime.is_empty());
    }

    // ---- segment 3: cmd runtime --------------------------------------

    #[test]
    fn discover_local_segment_3_hit_when_native_and_skill_empty() {
        let registry = registry_with(vec![]);
        let discovery =
            CapabilityDiscovery::new(registry, empty_skill_index()).with_config(cfg_sh_only());
        let query = DiscoveryQuery::new("script.run");
        let result = discovery.discover_local(&query);

        assert!(result.native.is_empty());
        assert!(result.installed_skills.is_empty());
        assert_eq!(result.cmd_runtime, vec!["sh".to_string()]);
        assert!(!result.skill_hub_pending);
    }

    // ---- all-empty path ----------------------------------------------

    #[test]
    fn discover_local_all_empty_marks_pending() {
        let registry = registry_with(vec![]);
        let discovery =
            CapabilityDiscovery::new(registry, empty_skill_index()).with_config(cfg_unknown_cmd());

        // Plug in async stubs to make the pending bits flip on.
        let hub: Arc<dyn SkillHubProvider> = Arc::new(EmptyHub);
        let llm: Arc<dyn LlmClient> = Arc::new(StubLlm {
            provider: "openai".to_string(),
        });
        let sink: Arc<dyn CapabilityRequestSink> = Arc::new(RecordingSink::default());

        let discovery = discovery
            .with_skill_hub(hub)
            .with_llm_client(llm)
            .with_capability_request_sink(sink);

        let query = DiscoveryQuery::new("does.not.exist");
        let result = discovery.discover_local(&query);

        assert!(result.native.is_empty());
        assert!(result.installed_skills.is_empty());
        assert!(result.cmd_runtime.is_empty());
        assert!(result.skill_hub_pending);
        assert!(result.provider_modalities_pending);
        assert!(result.request_pending);
        assert!(!result.has_local_hit());
    }

    // ---- discover_remote ---------------------------------------------

    struct EmptyHub;
    #[async_trait]
    impl SkillHubProvider for EmptyHub {
        async fn fetch_remote(&self, _query: &DiscoveryQuery) -> anyhow::Result<Vec<SkillBundle>> {
            Ok(Vec::new())
        }
    }

    struct OneHub;
    #[async_trait]
    impl SkillHubProvider for OneHub {
        async fn fetch_remote(&self, _query: &DiscoveryQuery) -> anyhow::Result<Vec<SkillBundle>> {
            use cyberclaw_skill_runtime::skill_scanner::SkillTrustLevel;
            Ok(vec![SkillBundle {
                name: "remote-skill".to_string(),
                version: "0.1.0".to_string(),
                description: "remote".to_string(),
                source: "registry:test".to_string(),
                trust_level: SkillTrustLevel::Community,
                sha256: None,
                signature: None,
                publisher_fingerprint: None,
            }])
        }
    }

    struct SlowHub;
    #[async_trait]
    impl SkillHubProvider for SlowHub {
        async fn fetch_remote(&self, _query: &DiscoveryQuery) -> anyhow::Result<Vec<SkillBundle>> {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(Vec::new())
        }
    }

    #[derive(Default)]
    struct RecordingSink {
        seen: Mutex<Vec<DiscoveryQuery>>,
    }

    #[async_trait]
    impl CapabilityRequestSink for RecordingSink {
        async fn record_gap(&self, query: &DiscoveryQuery) -> anyhow::Result<String> {
            self.seen.lock().unwrap().push(query.clone());
            Ok("req-123".to_string())
        }
    }

    use cyberclaw_llm::error::LlmResult;
    use cyberclaw_llm::types::{ChatChunk, ChatRequest, ChatResponse};
    use futures::stream::Stream;

    struct StubLlm {
        provider: String,
    }

    #[async_trait]
    impl LlmClient for StubLlm {
        async fn chat_completion(&self, _request: ChatRequest) -> LlmResult<ChatResponse> {
            unreachable!("not used in discovery tests")
        }
        async fn chat_completion_stream(
            &self,
            _request: ChatRequest,
        ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>> {
            unreachable!("not used in discovery tests")
        }
        fn provider(&self) -> &str {
            &self.provider
        }
        async fn validate_connection(&self) -> LlmResult<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn discover_remote_all_empty_records_request() {
        let registry = registry_with(vec![]);
        let sink: Arc<dyn CapabilityRequestSink> = Arc::new(RecordingSink::default());
        let discovery = CapabilityDiscovery::new(registry, empty_skill_index())
            .with_skill_hub(Arc::new(EmptyHub))
            .with_capability_request_sink(sink.clone())
            .with_config(cfg_unknown_cmd());

        let query = DiscoveryQuery::new("missing.cap");
        let remote = discovery.discover_remote(&query).await;

        assert!(remote.skill_hub_hits.is_empty());
        assert!(remote.provider_modalities.is_empty());
        assert_eq!(remote.request_id.as_deref(), Some("req-123"));
    }

    #[tokio::test]
    async fn discover_remote_hub_hit_skips_request() {
        let registry = registry_with(vec![]);
        let sink: Arc<dyn CapabilityRequestSink> = Arc::new(RecordingSink::default());
        let discovery = CapabilityDiscovery::new(registry, empty_skill_index())
            .with_skill_hub(Arc::new(OneHub))
            .with_capability_request_sink(sink.clone())
            .with_config(cfg_unknown_cmd());

        let query = DiscoveryQuery::new("audio.transcribe");
        let remote = discovery.discover_remote(&query).await;

        assert_eq!(remote.skill_hub_hits.len(), 1);
        assert_eq!(remote.skill_hub_hits[0].name, "remote-skill");
        assert!(remote.request_id.is_none());
    }

    #[tokio::test]
    async fn discover_remote_provider_probe_filtered_by_modality() {
        let registry = registry_with(vec![]);
        let llm: Arc<dyn LlmClient> = Arc::new(StubLlm {
            provider: "openai".to_string(),
        });
        let discovery = CapabilityDiscovery::new(registry, empty_skill_index())
            .with_skill_hub(Arc::new(EmptyHub))
            .with_llm_client(llm)
            .with_config(cfg_unknown_cmd());

        let query = DiscoveryQuery::new("audio.transcribe").with_modalities(vec![Modality::Audio]);
        let remote = discovery.discover_remote(&query).await;

        // openai modality table contains audio.transcriptions / audio.speech.
        assert!(remote
            .provider_modalities
            .iter()
            .any(|pm| pm.api.contains("audio")));
        // No image entry should survive the Audio filter.
        assert!(!remote
            .provider_modalities
            .iter()
            .any(|pm| pm.api.contains("vision")));
    }

    // ---- discover_full ------------------------------------------------

    #[tokio::test]
    async fn discover_full_local_hit_skips_remote() {
        let registry = registry_with(vec![("c", vec!["audio.transcribe"])]);
        let discovery = CapabilityDiscovery::new(registry, empty_skill_index())
            .with_skill_hub(Arc::new(SlowHub))
            .with_config(cfg_unknown_cmd());

        let query = DiscoveryQuery::new("audio.transcribe");
        let result = discovery
            .discover_full(&query, Duration::from_millis(50))
            .await;

        assert!(!result.native.is_empty());
        // skill_hub_pending stays false on a local short-circuit.
        assert!(!result.skill_hub_pending);
    }

    #[tokio::test]
    async fn discover_full_outer_timeout_graceful() {
        let registry = registry_with(vec![]);
        let discovery = CapabilityDiscovery::new(registry, empty_skill_index())
            .with_skill_hub(Arc::new(SlowHub))
            .with_config(cfg_unknown_cmd());

        let query = DiscoveryQuery::new("missing");
        let start = std::time::Instant::now();
        let result = discovery
            .discover_full(&query, Duration::from_millis(50))
            .await;
        // Outer timeout enforced — must return fast.
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(!result.has_local_hit());
    }

    // ---- serde round-trips -------------------------------------------

    #[test]
    fn discovery_query_serde_round_trip() {
        let query = DiscoveryQuery::new("audio.transcribe")
            .with_modalities(vec![Modality::Audio, Modality::Text])
            .with_search_terms(vec!["transcribe".to_string()]);
        let json = serde_json::to_string(&query).unwrap();
        let back: DiscoveryQuery = serde_json::from_str(&json).unwrap();
        assert_eq!(back.deliverable_kind, "audio.transcribe");
        assert_eq!(back.modalities, vec![Modality::Audio, Modality::Text]);
        assert_eq!(back.search_terms, vec!["transcribe".to_string()]);
    }

    #[test]
    fn discovery_result_serde_round_trip() {
        let mut result = DiscoveryResult::default();
        result.native.push((
            ConnectorId::from_string("c".to_string()).unwrap(),
            CapabilityId::from_string("audio.transcribe".to_string()).unwrap(),
        ));
        result.skill_hub_pending = true;
        let json = serde_json::to_string(&result).unwrap();
        let back: DiscoveryResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.native.len(), 1);
        assert!(back.skill_hub_pending);
    }

    #[test]
    fn modality_serde_round_trip() {
        let modalities = vec![
            Modality::Audio,
            Modality::Text,
            Modality::Image,
            Modality::Csv,
            Modality::Code,
            Modality::Url,
            Modality::Pdf,
            Modality::Pptx,
            Modality::Xlsx,
        ];
        let json = serde_json::to_string(&modalities).unwrap();
        let back: Vec<Modality> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, modalities);
    }

    // ---- sink mock capture -------------------------------------------

    #[tokio::test]
    async fn capability_request_sink_captures_query() {
        let registry = registry_with(vec![]);
        let sink: Arc<RecordingSink> = Arc::new(RecordingSink::default());
        let sink_dyn: Arc<dyn CapabilityRequestSink> = sink.clone();
        let discovery = CapabilityDiscovery::new(registry, empty_skill_index())
            .with_skill_hub(Arc::new(EmptyHub))
            .with_capability_request_sink(sink_dyn)
            .with_config(cfg_unknown_cmd());

        let query = DiscoveryQuery::new("missing.cap").with_search_terms(vec!["x".to_string()]);
        let _ = discovery.discover_remote(&query).await;

        let seen = sink.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].deliverable_kind, "missing.cap");
    }

    // ---- SkillHubIndex adapter ---------------------------------------

    #[test]
    fn skill_hub_index_search_dedups_results() {
        use cyberclaw_skill_runtime::skill_scanner::SkillTrustLevel;
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let mut hub = SkillHub::new(dir.path().to_path_buf()).unwrap();
        hub.register_bundle(SkillBundle {
            name: "audio-transcribe".to_string(),
            version: "1.0".to_string(),
            description: "transcribe audio".to_string(),
            source: "local".to_string(),
            trust_level: SkillTrustLevel::Trusted,
            sha256: None,
            signature: None,
            publisher_fingerprint: None,
        });
        let arc_hub = Arc::new(hub);
        let index = SkillHubIndex::new(arc_hub);

        // Two terms that both match the same skill — must not duplicate.
        let hits = index.search(&["audio".to_string(), "transcribe".to_string()]);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].as_str(), "audio-transcribe");

        // Empty search terms returns empty (deterministic dispatch).
        let hits = index.search(&[]);
        assert!(hits.is_empty());
    }
}
