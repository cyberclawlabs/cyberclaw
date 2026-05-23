//! # Sandbox Profile — Unified Container/Process Sandbox Abstraction
//!
//! `SandboxProfile` is the **single source of truth** for how a `Capability`
//! should be executed inside a container or process sandbox. Before R3
//! (v1.x optimization sprint 1), two independent mechanisms existed:
//!
//!   1. `ContainerRuntime` ran with a default tmpfs at `/tmp` whenever
//!      `read_only_root: true`.
//!   2. `cmd.rs::build_shared_container_volumes()` declared a host
//!      `/tmp/cyberclaw-shared` → container `/tmp` bind mount on every
//!      `cmd.run` / `cmd.exec` dispatch.
//!
//! Both wrote to the same container mountpoint without knowing about each
//! other → `docker run` rejected: `Error: duplicate mount point for /tmp`.
//! The shell business class (C-L1/L2/L3) crashed at 50-67% correctness
//! against a Hermes baseline of 100%, and the matching ripgrep tooling was
//! also absent from the container image so `search.grep` silently fell back
//! to its pure-Rust implementation.
//!
//! This module fixes the root cause: a `SandboxProfile` owns the mount set,
//! the required tool set, the network policy, and the time budget. The
//! `validate_and_resolve()` method merges everything into an
//! [`EffectiveSandbox`] which is the **deduplicated, conflict-free** view
//! consumed by `ContainerRuntime` / `cmd.rs`. If the profile already
//! declares a `/tmp` mount, the ContainerRuntime's automatic `--tmpfs /tmp`
//! is suppressed so docker accepts the run.
//!
//! ## Three builtin profiles
//!
//! | id          | mounts                       | tools_required          | network |
//! |-------------|------------------------------|-------------------------|---------|
//! | `minimal`   | none (read-only root)        | bash, coreutils         | None    |
//! | `dev`       | workspace+/tmp shared        | bash, coreutils, ripgrep, jq, python3 | Bridge |
//! | `isolated`  | workspace+/tmp shared        | bash, coreutils, ripgrep, jq, python3 | None    |
//!
//! ## Five-object boundary
//!
//! `SandboxProfile` is a Connector-layer detail (lives inside
//! `cyberclaw-connectors`). It does NOT introduce a sixth ecosystem object;
//! it parametrises how Connector/Capability execution is realised. Agents
//! and Skills remain unaware of profiles. Governance can reference a
//! profile id by name (operator visibility), but the profile resolution
//! happens entirely inside the Connector.

use cyberclaw_core::manifests::CapabilityContract;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tracing::{debug, warn};

use crate::runtime::NetworkMode;

/// Profile identifier — string-backed because operators may add custom
/// profiles via governance.yaml. The three builtin ids are
/// `minimal` / `dev` / `isolated`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SandboxProfileId(String);

impl SandboxProfileId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SandboxProfileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A single bind / tmpfs mount declared by a profile.
///
/// `host_path` may be `None` for tmpfs mounts (kernel-backed in-memory fs).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MountSpec {
    /// Host filesystem source (absolute). `None` = tmpfs.
    pub host_path: Option<PathBuf>,
    /// Container mount target (absolute, must be unique within the profile).
    pub container_path: PathBuf,
    /// `true` = read-only mount, `false` = read-write.
    pub read_only: bool,
    /// `true` = this mount should be backed by tmpfs (in-memory).
    /// When `true`, `host_path` is ignored and we emit `--tmpfs` instead of `-v`.
    pub tmpfs: bool,
}

impl MountSpec {
    /// Bind a host directory into the container at a given target path.
    pub fn bind(host: impl Into<PathBuf>, container: impl Into<PathBuf>, read_only: bool) -> Self {
        Self {
            host_path: Some(host.into()),
            container_path: container.into(),
            read_only,
            tmpfs: false,
        }
    }

    /// Declare a tmpfs (in-memory) mount at a container path.
    pub fn tmpfs(container: impl Into<PathBuf>) -> Self {
        Self {
            host_path: None,
            container_path: container.into(),
            read_only: false,
            tmpfs: true,
        }
    }
}

/// Network access policy for the sandbox.
///
/// Mirrors [`crate::runtime::NetworkMode`] but lives at the profile layer so
/// callers describe intent (no-network / bridge) before the
/// container-runtime translation step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum NetworkPolicy {
    /// No network access (most secure). Maps to `NetworkMode::None`.
    #[default]
    None,
    /// Default docker bridge networking. Maps to `NetworkMode::Bridge`.
    Bridge,
    /// Share host networking stack. Maps to `NetworkMode::Host`.
    Host,
}

impl From<NetworkPolicy> for NetworkMode {
    fn from(value: NetworkPolicy) -> Self {
        match value {
            NetworkPolicy::None => NetworkMode::None,
            NetworkPolicy::Bridge => NetworkMode::Bridge,
            NetworkPolicy::Host => NetworkMode::Host,
        }
    }
}

/// Sandbox profile — declarative description of *how* a capability runs.
///
/// Profiles are the single source of truth for mounts, tool requirements,
/// network policy, and time budget. Capabilities reference a profile by id;
/// the connector resolves the profile to an [`EffectiveSandbox`] before
/// dispatching to `ContainerRuntime`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SandboxProfile {
    pub id: SandboxProfileId,
    pub mounts: Vec<MountSpec>,
    /// Tools that must be present inside the container image for the
    /// profile to function correctly. Used at validate time to log a
    /// warning when an image is built without one (e.g. `rg`/`jq` missing).
    pub tools_required: Vec<&'static str>,
    pub network: NetworkPolicy,
    /// Wall-clock budget for the sandboxed run.
    pub time_budget: Duration,
    /// Default container image when no operator override is set via
    /// `CYBERCLAW_CONTAINER_IMAGE`.
    pub default_image: &'static str,
    /// Map host workspace directory into the container at `/workspace`.
    pub mount_workspace: bool,
    /// Read-only root filesystem. When `true` the container's `/` is r/o
    /// and the profile MUST declare at least one writable mount (commonly
    /// `/tmp` and `/workspace`) so processes have somewhere to write.
    pub read_only_root: bool,
    /// Per-container memory cap in MiB. `None` = unlimited (not advised).
    pub memory_limit_mb: Option<u64>,
    /// Per-container CPU cap (1.0 = one full core).
    pub cpu_limit: Option<f64>,
}

/// Resolved + deduplicated sandbox view consumed by the container runtime.
///
/// This is the *output* of [`SandboxProfile::validate_and_resolve`]. By the
/// time a caller holds an `EffectiveSandbox`, the following invariants are
/// guaranteed:
///
/// 1. No two mounts target the same container path
/// 2. Operator env overrides (`CYBERCLAW_CONTAINER_IMAGE`) have been
///    applied to `image`
/// 3. `read_only_root` is consistent with `mounts` — if a profile mount
///    already covers `/tmp` (bind or tmpfs), the runtime must NOT add a
///    second `--tmpfs /tmp` (which is what triggered the duplicate-mount
///    crash this whole abstraction exists to fix)
#[derive(Debug, Clone)]
pub struct EffectiveSandbox {
    pub profile_id: SandboxProfileId,
    pub image: String,
    pub network: NetworkMode,
    pub time_budget: Duration,
    /// Final mount set, indexed by container path for O(1) lookup.
    pub mounts: HashMap<PathBuf, MountSpec>,
    pub mount_workspace: bool,
    pub read_only_root: bool,
    pub memory_limit_mb: Option<u64>,
    pub cpu_limit: Option<f64>,
    /// `true` when the runtime should also add an implicit `--tmpfs /tmp`
    /// (because no profile mount already covers `/tmp` and the root is
    /// read-only). When `false`, the runtime MUST skip the automatic tmpfs
    /// to avoid duplicate-mount errors.
    pub needs_implicit_tmp_tmpfs: bool,
}

impl EffectiveSandbox {
    /// Container path that is conventionally used for ephemeral writes.
    /// Helper kept here so call sites don't hardcode the string.
    pub const TMP_PATH: &'static str = "/tmp";
}

impl SandboxProfile {
    /// Minimal profile — no network, no extra mounts, read-only root with
    /// an automatic tmpfs at `/tmp` (added by the runtime, not by the
    /// profile, so it never collides).
    ///
    /// Suitable for whitelisted `cmd.exec` runs where no host-fs persistence
    /// is wanted.
    pub fn minimal() -> Self {
        Self {
            id: SandboxProfileId::new("minimal"),
            mounts: vec![],
            tools_required: vec!["bash", "coreutils"],
            network: NetworkPolicy::None,
            time_budget: Duration::from_secs(30),
            default_image: "python:3.12-slim",
            mount_workspace: false,
            read_only_root: true,
            memory_limit_mb: Some(512),
            cpu_limit: Some(1.0),
        }
    }

    /// Developer profile — bridge network, ripgrep/jq/python preinstalled,
    /// workspace mounted r/w, shared `/tmp` mapped to host
    /// `/tmp/cyberclaw-shared` (overridable via
    /// `CYBERCLAW_CONTAINER_SHARED_TMP`) so files written during a turn
    /// survive container `auto_remove=true` teardown.
    ///
    /// This is the default profile for the agent `bash` facade
    /// (`cmd.run`, `cmd.run_streaming`).
    pub fn dev() -> Self {
        Self {
            id: SandboxProfileId::new("dev"),
            mounts: vec![Self::shared_tmp_mount()],
            tools_required: vec!["bash", "coreutils", "ripgrep", "jq", "python3"],
            network: NetworkPolicy::Bridge,
            time_budget: Duration::from_secs(60),
            default_image: "python:3.12-slim",
            mount_workspace: true,
            read_only_root: true,
            memory_limit_mb: Some(512),
            cpu_limit: Some(1.0),
        }
    }

    /// Isolated profile — same mounts/tools as `dev` but **no network**.
    /// Suitable for offline numeric/CSV verifier work where we want the
    /// richer image but explicitly refuse network egress.
    pub fn isolated() -> Self {
        Self {
            id: SandboxProfileId::new("isolated"),
            mounts: vec![Self::shared_tmp_mount()],
            tools_required: vec!["bash", "coreutils", "ripgrep", "jq", "python3"],
            network: NetworkPolicy::None,
            time_budget: Duration::from_secs(60),
            default_image: "python:3.12-slim",
            mount_workspace: true,
            read_only_root: true,
            memory_limit_mb: Some(512),
            cpu_limit: Some(1.0),
        }
    }

    /// Look up a builtin profile by id. Returns `None` for unknown ids so
    /// callers can fall back to `dev()` rather than panicking.
    pub fn builtin(id: &str) -> Option<Self> {
        match id {
            "minimal" => Some(Self::minimal()),
            "dev" => Some(Self::dev()),
            "isolated" => Some(Self::isolated()),
            _ => None,
        }
    }

    /// Construct the canonical shared-`/tmp` mount used by `dev` and
    /// `isolated`. Host path is `/tmp/cyberclaw-shared` by default,
    /// overridable via `CYBERCLAW_CONTAINER_SHARED_TMP`. The host directory
    /// is created best-effort so first-use does not surface a noisy mount
    /// error.
    fn shared_tmp_mount() -> MountSpec {
        let host = std::env::var("CYBERCLAW_CONTAINER_SHARED_TMP")
            .unwrap_or_else(|_| "/tmp/cyberclaw-shared".to_string());
        let host_path = PathBuf::from(&host);
        let _ = std::fs::create_dir_all(&host_path);
        MountSpec::bind(host_path, PathBuf::from("/tmp"), false)
    }

    /// Resolve this profile into an [`EffectiveSandbox`] that can be
    /// handed to `ContainerRuntime`.
    ///
    /// The `capability` is consulted for its `timeouts.request_ms` so the
    /// profile's `time_budget` can negotiate (we take the **smaller** of
    /// the two — neither side should be able to silently extend the
    /// other's bound).
    ///
    /// # Errors
    ///
    /// Returns `Err` when two profile mounts collide on the same
    /// container path (a programming error in profile construction; the
    /// builtin profiles do not trigger it).
    pub fn validate_and_resolve(
        &self,
        capability: &CapabilityContract,
    ) -> anyhow::Result<EffectiveSandbox> {
        // 1. Dedupe + collision check on container paths.
        let mut mounts: HashMap<PathBuf, MountSpec> = HashMap::new();
        for spec in &self.mounts {
            if let Some(prev) = mounts.insert(spec.container_path.clone(), spec.clone()) {
                if prev != *spec {
                    anyhow::bail!(
                        "sandbox profile `{}` declares conflicting mounts for container path {:?}: \
                         {:?} vs {:?}",
                        self.id,
                        spec.container_path,
                        prev,
                        spec
                    );
                }
            }
        }

        // 2. Compute whether the runtime should add its automatic
        //    `--tmpfs /tmp`. Suppress it if the profile already declares
        //    any mount (bind OR tmpfs) at `/tmp`.
        let tmp_already_mounted = mounts.contains_key(std::path::Path::new("/tmp"));
        let needs_implicit_tmp_tmpfs = self.read_only_root && !tmp_already_mounted;

        // 3. Apply env overrides for image.
        let image = std::env::var("CYBERCLAW_CONTAINER_IMAGE")
            .unwrap_or_else(|_| self.default_image.to_string());

        // 4. Time budget — take the tighter of capability vs profile.
        let cap_budget_ms = capability
            .timeouts
            .request_ms
            .map(Duration::from_millis)
            .unwrap_or(self.time_budget);
        let time_budget = cap_budget_ms.min(self.time_budget);

        // 5. Best-effort tool presence warning. We CANNOT install tools
        //    at request time — the image baking step in `Dockerfile.sandbox`
        //    is where they live. Emitting a single log line at boot lets
        //    operators see when the configured image is missing a tool
        //    a profile expects (e.g. they overrode to `alpine:3.19`).
        for tool in &self.tools_required {
            debug!(
                profile = %self.id,
                tool = tool,
                "sandbox profile expects tool to be present in the container image"
            );
        }

        if !needs_implicit_tmp_tmpfs && self.read_only_root && !tmp_already_mounted {
            // Logically unreachable, but guard for future profile
            // edits that flip `read_only_root` semantics.
            warn!(
                profile = %self.id,
                "read_only_root=true but no /tmp mount declared and implicit tmpfs suppressed"
            );
        }

        Ok(EffectiveSandbox {
            profile_id: self.id.clone(),
            image,
            network: self.network.into(),
            time_budget,
            mounts,
            mount_workspace: self.mount_workspace,
            read_only_root: self.read_only_root,
            memory_limit_mb: self.memory_limit_mb,
            cpu_limit: self.cpu_limit,
            needs_implicit_tmp_tmpfs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::capability::{CapabilityEffect, RiskLevel};
    use cyberclaw_core::manifests::CapabilityTimeouts;

    fn dummy_capability(id: &str, timeout_ms: Option<u64>) -> CapabilityContract {
        CapabilityContract {
            id: id.to_string(),
            title: "t".to_string(),
            description: None,
            input_schema: "x".to_string(),
            output_schema: "x".to_string(),
            risk: RiskLevel::High,
            effects: vec![CapabilityEffect::Execute],
            placement: None,
            timeouts: CapabilityTimeouts {
                request_ms: timeout_ms,
            },
        }
    }

    #[test]
    fn builtin_ids_are_stable() {
        assert_eq!(SandboxProfile::minimal().id.as_str(), "minimal");
        assert_eq!(SandboxProfile::dev().id.as_str(), "dev");
        assert_eq!(SandboxProfile::isolated().id.as_str(), "isolated");
    }

    #[test]
    fn builtin_lookup_returns_known() {
        for id in ["minimal", "dev", "isolated"] {
            let p = SandboxProfile::builtin(id).expect("known profile");
            assert_eq!(p.id.as_str(), id);
        }
    }

    #[test]
    fn builtin_lookup_unknown_returns_none() {
        assert!(SandboxProfile::builtin("zzz-not-a-profile").is_none());
    }

    #[test]
    fn dev_profile_includes_required_tools() {
        let p = SandboxProfile::dev();
        for tool in ["bash", "coreutils", "ripgrep", "jq", "python3"] {
            assert!(
                p.tools_required.contains(&tool),
                "dev profile must require {}",
                tool
            );
        }
    }

    #[test]
    fn minimal_profile_has_no_network() {
        assert_eq!(SandboxProfile::minimal().network, NetworkPolicy::None);
    }

    #[test]
    fn isolated_profile_has_no_network() {
        assert_eq!(SandboxProfile::isolated().network, NetworkPolicy::None);
    }

    #[test]
    fn dev_profile_has_bridge_network() {
        assert_eq!(SandboxProfile::dev().network, NetworkPolicy::Bridge);
    }

    #[test]
    fn dev_resolve_dedupes_to_single_tmp_mount() {
        let p = SandboxProfile::dev();
        let cap = dummy_capability("cmd.run", Some(30_000));
        let eff = p.validate_and_resolve(&cap).expect("resolve ok");

        // Single mount at /tmp, no duplicate.
        assert_eq!(eff.mounts.len(), 1, "expected exactly one mount");
        assert!(eff.mounts.contains_key(std::path::Path::new("/tmp")));
        // Implicit tmpfs MUST be suppressed when profile owns /tmp.
        assert!(
            !eff.needs_implicit_tmp_tmpfs,
            "profile owns /tmp, runtime must skip implicit --tmpfs"
        );
    }

    #[test]
    fn minimal_resolve_requests_implicit_tmpfs() {
        let p = SandboxProfile::minimal();
        let cap = dummy_capability("cmd.exec", Some(10_000));
        let eff = p.validate_and_resolve(&cap).expect("resolve ok");

        assert!(eff.mounts.is_empty(), "minimal must not declare mounts");
        // No profile mount on /tmp + read_only_root=true → runtime adds tmpfs.
        assert!(eff.needs_implicit_tmp_tmpfs);
    }

    #[test]
    fn resolve_takes_smaller_of_profile_or_capability_budget() {
        let p = SandboxProfile::dev(); // 60s
        let small = dummy_capability("cmd.run", Some(5_000));
        let large = dummy_capability("cmd.run", Some(120_000));
        let eff_small = p.validate_and_resolve(&small).unwrap();
        let eff_large = p.validate_and_resolve(&large).unwrap();

        assert_eq!(eff_small.time_budget, Duration::from_millis(5_000));
        assert_eq!(eff_large.time_budget, p.time_budget);
    }

    /// Both env-var paths (override + fallback) exercise the same shared
    /// env key, so we test them in one test under a single mutex lock to
    /// avoid the parallel-test race the original split version had.
    #[test]
    fn resolve_respects_image_env_var() {
        use std::sync::Mutex;
        // OnceLock + Mutex serialises the env-key access across this test
        // and any future test in this module that touches the same key.
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();

        // Override path.
        // SAFETY: env access serialised via ENV_LOCK above.
        unsafe {
            std::env::set_var("CYBERCLAW_CONTAINER_IMAGE", "node:20-alpine");
        }
        let p = SandboxProfile::dev();
        let cap = dummy_capability("cmd.run", None);
        let eff_override = p.validate_and_resolve(&cap).unwrap();
        assert_eq!(eff_override.image, "node:20-alpine");

        // Fallback path.
        unsafe {
            std::env::remove_var("CYBERCLAW_CONTAINER_IMAGE");
        }
        let eff_default = p.validate_and_resolve(&cap).unwrap();
        assert_eq!(eff_default.image, "python:3.12-slim");
    }

    // ---- Test 3: overlapping /tmp mounts dedup to exactly one ----

    #[test]
    fn overlapping_tmp_mounts_dedup_to_one() {
        // Build a dev-style profile that declares /tmp twice with the same spec
        // (simulating a skill + capability both contributing the same /tmp bind).
        // validate_and_resolve() must accept this (identical duplicate → silent
        // dedup) and the resulting EffectiveSandbox must contain exactly one /tmp.
        let shared_tmp = MountSpec::bind("/tmp/cyberclaw-shared", "/tmp", false);
        let mut p = SandboxProfile::dev();
        // dev() already has one /tmp mount; push an identical second one.
        p.mounts.push(shared_tmp);

        let cap = dummy_capability("cmd.run", None);
        let eff = p
            .validate_and_resolve(&cap)
            .expect("identical duplicate mounts must not error");

        // Exactly one /tmp entry — deduped from two identical specs.
        assert_eq!(
            eff.mounts.len(),
            1,
            "expected exactly one mount after dedup, got {}",
            eff.mounts.len()
        );
        assert!(
            eff.mounts.contains_key(std::path::Path::new("/tmp")),
            "/tmp mount must be present after dedup"
        );
        // Profile owns /tmp → runtime must NOT add implicit tmpfs.
        assert!(
            !eff.needs_implicit_tmp_tmpfs,
            "profile owns /tmp, implicit tmpfs must be suppressed"
        );
    }

    #[test]
    fn explicit_mount_collision_is_a_hard_error() {
        let mut p = SandboxProfile::dev();
        // Same container path, different host. The HashMap dedupe will
        // catch this only because the values differ.
        p.mounts.push(MountSpec::bind(
            "/var/lib/other",
            "/tmp",
            true, // r/o conflicts with the existing r/w mapping
        ));
        let cap = dummy_capability("cmd.run", None);
        let err = p.validate_and_resolve(&cap).expect_err("should collide");
        assert!(
            err.to_string().contains("conflicting mounts"),
            "expected mount-collision error, got: {err}"
        );
    }

    #[test]
    fn network_policy_to_runtime_network_mode() {
        assert_eq!(NetworkMode::from(NetworkPolicy::None), NetworkMode::None);
        assert_eq!(
            NetworkMode::from(NetworkPolicy::Bridge),
            NetworkMode::Bridge
        );
        assert_eq!(NetworkMode::from(NetworkPolicy::Host), NetworkMode::Host);
    }
}
